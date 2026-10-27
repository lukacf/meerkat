//! Every Meta composition that installs agent mob tools gives `fork_off`
//! and `council` the DETACHED route.
//!
//! Detached completion needs the session service's canonical runtime and a
//! bound continuation owner. Without that composition, `fork_off` and
//! `council` fall back to blocking for the child's whole run.
//!
//! `MobMcpState::new` takes its adapter from the session service it is given
//! (`MobSessionService::acquire_runtime_adapter`), captured when Meta
//! installs the agent mob tools. Constructor checks pin the detached route
//! and shared runtime. The actual-tool fixtures then prove that MobRuntime's
//! delivery owner applies a member-addressed completion once, even with no
//! callback jobs from which an older host could discover that member.

#![allow(clippy::expect_used, clippy::panic)]

use std::sync::Arc;

use meerkat::{Config, FactoryAgentBuilder};
use meerkat_session::PersistentSessionService;

use crate::mob_handle_runtime::{CapabilityFlags, MobBootstrapSpec, MobRuntimeDelivery};

fn definition(mob_id: &str) -> meerkat_mob::MobDefinition {
    meerkat_mob::MobDefinition::from_toml(&format!(
        r#"
[mob]
id = "{mob_id}"

[profiles.general]
model = "gpt-5.5"

[profiles.general.tools]
comms = true
"#
    ))
    .expect("mob definition")
}

/// The route is detached and runs on the machine the runtime uses.
fn assert_detached_route_on_runtime_machine(spec: &MobBootstrapSpec, composition: &str) {
    let state = spec
        .agent_mob_mcp_state
        .as_ref()
        .unwrap_or_else(|| panic!("{composition}: agent mob tools must be installed"));
    assert!(
        Arc::ptr_eq(
            spec.storage
                .forked_participant_store()
                .expect("fixture root participant custody"),
            &state.forked_participant_store_for_tests(),
        ),
        "{composition}: councils must read the root mob's exact capability store"
    );
    assert_eq!(
        state.detached_delivery_blocked_because(),
        None,
        "{composition}: fork_off/council must deliver detached, not block"
    );
    let route = state
        .session_service()
        .acquire_runtime_adapter(None)
        .unwrap_or_else(|error| {
            panic!("{composition}: acquire the detached route runtime: {error}")
        })
        .unwrap_or_else(|| panic!("{composition}: the session service exposes no runtime"));
    // The canonical machine `MobRuntime` hands to `MobBuilder`, acquired
    // from the session service with the spec's explicit adapter.
    let runtime = spec
        .session_service
        .acquire_runtime_adapter(spec.runtime_adapter.clone())
        .unwrap_or_else(|error| panic!("{composition}: acquire the runtime machine: {error}"))
        .unwrap_or_else(|| panic!("{composition}: the runtime has no machine"));
    assert!(
        Arc::ptr_eq(&route, &runtime),
        "{composition}: detached delivery must use the machine that hosts the sessions"
    );
}

/// `rpc_gateway --persistent` (a downstream app) and `meta_gateway` with console
/// voice: a concrete persistent session service, an explicit persistent
/// machine wired with `with_session_runtime_adapter`, then
/// `with_agent_mob_tools`, in that order.
#[tokio::test]
async fn gateway_persistent_composition_delivers_detached() {
    let temp = tempfile::tempdir().expect("temp dir");
    let state = temp.path().join("state");
    std::fs::create_dir_all(&state).expect("state dir");
    // Both gateways open Core's native pair on runtime.sqlite.
    let (session_store, runtime_store) = crate::mob_handle_runtime::open_native_session_pair(
        &state,
        &crate::storage_provider::default_meerkat_level_realm(),
    )
    .expect("native session pair");
    let blob_store: Arc<dyn meerkat_core::BlobStore> =
        Arc::new(meerkat_store::MemoryBlobStore::new());
    let factory = meerkat::AgentFactory::new(&state).comms(true);
    let mut builder = FactoryAgentBuilder::new(factory, Config::default());
    builder.default_session_store = Some(Arc::new(meerkat_store::StoreAdapter::new(
        session_store.clone(),
    )));
    builder.default_blob_store = Some(blob_store.clone());
    let jobs: Arc<dyn meerkat::DetachedJobStore> = Arc::new(meerkat::MemoryDetachedJobStore::new());
    builder.default_detached_job_store = Some(Arc::clone(&jobs));
    let delivery = MobRuntimeDelivery::new(Arc::clone(&runtime_store), jobs);
    let mob_tools_slot = Arc::clone(&builder.default_mob_tools);
    let adapter = Arc::new(
        meerkat_runtime::MeerkatMachine::persistent(
            Arc::clone(&runtime_store),
            Arc::clone(&blob_store),
        )
        .expect("acquire the detached delivery fixture runtime machine"),
    );
    let service = Arc::new(PersistentSessionService::new(
        builder,
        16,
        session_store,
        runtime_store,
        blob_store,
    ));
    let mut spec = MobBootstrapSpec::new(
        definition("route-gateway-persistent"),
        meerkat_mob::MobStorage::in_memory(),
        service,
    )
    .with_session_runtime_adapter(adapter.clone())
    .expect("acquire the fixture session runtime owner")
    .with_runtime_delivery(delivery)
    .with_agent_mob_tools(mob_tools_slot)
    .expect("install the detached delivery fixture's agent mob tools");
    spec.runtime_adapter = Some(adapter);
    assert_detached_route_on_runtime_machine(&spec, "gateway persistent composition");
}

/// `rpc_gateway`'s and `meta_gateway`'s process-local modes: a persistent
/// session service over in-memory stores with an explicit machine.
#[tokio::test]
async fn gateway_ephemeral_session_composition_delivers_detached() {
    let temp = tempfile::tempdir().expect("temp dir");
    let session_store: Arc<dyn meerkat::SessionStore> = Arc::new(meerkat::MemoryStore::new());
    let runtime_store: Arc<dyn meerkat_runtime::RuntimeStore> =
        Arc::new(meerkat_runtime::InMemoryRuntimeStore::new());
    let blob_store: Arc<dyn meerkat_core::BlobStore> =
        Arc::new(meerkat_store::MemoryBlobStore::new());
    let factory = meerkat::AgentFactory::new(temp.path()).comms(true);
    let mut builder = FactoryAgentBuilder::new(factory, Config::default());
    builder.default_session_store = Some(Arc::new(meerkat_store::StoreAdapter::new(
        session_store.clone(),
    )));
    builder.default_blob_store = Some(blob_store.clone());
    let jobs: Arc<dyn meerkat::DetachedJobStore> = Arc::new(meerkat::MemoryDetachedJobStore::new());
    builder.default_detached_job_store = Some(Arc::clone(&jobs));
    let delivery = MobRuntimeDelivery::new(Arc::clone(&runtime_store), jobs);
    let mob_tools_slot = Arc::clone(&builder.default_mob_tools);
    let adapter = Arc::new(
        meerkat_runtime::MeerkatMachine::persistent(
            Arc::clone(&runtime_store),
            Arc::clone(&blob_store),
        )
        .expect("acquire the detached delivery fixture runtime machine"),
    );
    let service = Arc::new(PersistentSessionService::new(
        builder,
        16,
        session_store,
        runtime_store,
        blob_store,
    ));
    let mut spec = MobBootstrapSpec::new(
        definition("route-gateway-ephemeral"),
        meerkat_mob::MobStorage::in_memory(),
        service,
    )
    .with_session_runtime_adapter(adapter.clone())
    .expect("acquire the fixture session runtime owner")
    .with_runtime_delivery(delivery)
    .with_agent_mob_tools(mob_tools_slot)
    .expect("install the detached delivery fixture's agent mob tools");
    spec.runtime_adapter = Some(adapter);
    assert_detached_route_on_runtime_machine(&spec, "gateway ephemeral-session composition");
}

/// `MobBootstrapSpec::persistent`, the persistent `UnifiedRuntimeBuilder`
/// path (identity-first library hosts).
#[tokio::test]
async fn library_persistent_constructor_delivers_detached() {
    let temp = tempfile::tempdir().expect("temp dir");
    let spec = MobBootstrapSpec::persistent(
        definition("route-library-persistent"),
        meerkat_mob::MobStorage::in_memory(),
        crate::mob_composition_manifest::MobStorageProvenance::declared_ephemeral(),
        temp.path().join("state"),
        16,
    )
    .await
    .expect("persistent spec");
    assert_detached_route_on_runtime_machine(&spec, "library persistent constructor");
}

/// The runtime-backed ephemeral constructor, the ephemeral
/// `UnifiedRuntimeBuilder` path.
#[tokio::test]
async fn library_runtime_backed_ephemeral_constructor_delivers_detached() {
    let temp = tempfile::tempdir().expect("temp dir");
    let spec = MobBootstrapSpec::ephemeral_runtime_backed_inner(
        definition("route-library-runtime-backed"),
        meerkat_mob::MobStorage::in_memory(),
        temp.path().to_path_buf(),
        16,
        None,
        None,
        None,
        None,
        CapabilityFlags::default(),
        None,
        None,
    )
    .expect("build the runtime-backed ephemeral detached delivery spec");
    assert_detached_route_on_runtime_machine(&spec, "library runtime-backed ephemeral constructor");
}

/// `MobBootstrapSpec::ephemeral`, the plain ephemeral library constructor.
#[tokio::test]
async fn library_ephemeral_constructor_delivers_detached() {
    let temp = tempfile::tempdir().expect("temp dir");
    let spec = MobBootstrapSpec::ephemeral(
        definition("route-library-ephemeral"),
        meerkat_mob::MobStorage::in_memory(),
        temp.path().to_path_buf(),
        16,
        None,
    )
    .expect("build the ephemeral detached delivery spec");
    assert_detached_route_on_runtime_machine(&spec, "library ephemeral constructor");
}

/// Exercise the host composition through real native tools and sessions. Only
/// provider responses are scripted; no test code submits or drains completion
/// inputs. The callback job store stays empty, so enumerating callback origins
/// cannot discover these member-addressed continuation rows.
mod completion_delivery {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    use futures::StreamExt as _;
    use meerkat_client::types::LlmStream;
    use meerkat_client::{LlmClient, LlmError, LlmEvent, LlmRequest};
    use meerkat_core::event::BackgroundJobTerminalStatus;
    use meerkat_core::types::{SystemNoticeBlock, SystemNoticeKind, ToolCallView};
    use meerkat_core::{AgentToolDispatcher, Message, SessionId};
    use meerkat_mob::{AgentIdentity, MobSessionService, SpawnMemberSpec};
    use serde_json::{Value, json};

    use super::*;
    use crate::mob_handle_runtime::{ContinuationSourceAnnouncer, MobBootstrapOptions, MobRuntime};

    const WAIT: Duration = Duration::from_mins(1);
    const CHILD_TASK: &str = "META-HELD-FORK-TASK";
    const CHILD_REPLY: &str = "META-FORK-RESULT";
    const COUNCIL_SUMMARY: &str = "META-COUNCIL-SUMMARY";
    const FOLLOW_UP: &str = "META-OWNER-FOLLOW-UP";
    const ANNOUNCED: &str = "META-ANNOUNCED-CONTINUATION";

    struct TurnGate {
        open: tokio::sync::watch::Sender<bool>,
        entered: tokio::sync::watch::Sender<usize>,
    }

    impl TurnGate {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                open: tokio::sync::watch::Sender::new(false),
                entered: tokio::sync::watch::Sender::new(0),
            })
        }

        async fn wait(&self) {
            let mut open = self.open.subscribe();
            self.entered.send_modify(|count| *count += 1);
            while !*open.borrow_and_update() {
                open.changed().await.expect("gate remains owned");
            }
        }

        async fn wait_entered(&self, count: usize) {
            let mut entered = self.entered.subscribe();
            tokio::time::timeout(WAIT, entered.wait_for(|seen| *seen >= count))
                .await
                .expect("child provider turn reaches the gate")
                .expect("gate remains owned");
        }

        fn release(&self) {
            self.open.send_replace(true);
        }
    }

    struct ReleaseOnDrop(Arc<TurnGate>);

    impl Drop for ReleaseOnDrop {
        fn drop(&mut self) {
            self.0.release();
        }
    }

    #[derive(Default)]
    struct RequestLog {
        messages: Mutex<Vec<Vec<Message>>>,
        recorded: tokio::sync::watch::Sender<usize>,
    }

    impl RequestLog {
        fn record(&self, request: &LlmRequest) {
            self.messages
                .lock()
                .expect("request log")
                .push(request.messages.clone());
            self.recorded.send_modify(|count| *count += 1);
        }

        fn requests_with_completion(&self, job_id: &str) -> usize {
            self.messages
                .lock()
                .expect("request log")
                .iter()
                .filter(|messages| !completion_records(messages, job_id).is_empty())
                .count()
        }

        fn assert_follow_up_retains_completion(&self, job_id: &str) {
            let messages = self.messages.lock().expect("request log");
            let follow_ups = messages
                .iter()
                .filter(|messages| {
                    messages
                        .iter()
                        .rev()
                        .find_map(|message| match message {
                            Message::User(user) => Some(user.text_content()),
                            _ => None,
                        })
                        .as_deref()
                        == Some(FOLLOW_UP)
                })
                .collect::<Vec<_>>();
            assert_eq!(follow_ups.len(), 1, "one explicit owner follow-up request");
            assert_eq!(
                completion_records(follow_ups[0], job_id).len(),
                1,
                "the follow-up itself carries the one persisted completion"
            );
        }

        /// Requests whose newest user message carries `text`: the turns that
        /// input itself started.
        fn turns_started_by(&self, text: &str) -> usize {
            self.messages
                .lock()
                .expect("request log")
                .iter()
                .filter(|messages| {
                    messages
                        .iter()
                        .rev()
                        .find_map(|message| match message {
                            Message::User(user) => Some(user.text_content()),
                            _ => None,
                        })
                        .is_some_and(|newest| newest.contains(text))
                })
                .count()
        }

        async fn wait_for_turn_started_by(&self, text: &str) {
            let mut recorded = self.recorded.subscribe();
            tokio::time::timeout(WAIT, recorded.wait_for(|_| self.turns_started_by(text) > 0))
                .await
                .expect("the admitted continuation starts its owner's turn")
                .expect("request log remains owned");
        }

        async fn wait_for_completion_request(&self, job_id: &str) {
            let mut recorded = self.recorded.subscribe();
            tokio::time::timeout(
                WAIT,
                recorded.wait_for(|_| self.requests_with_completion(job_id) > 0),
            )
            .await
            .expect("completion wakes its owner")
            .expect("request log remains owned");
        }
    }

    struct CompletionClient {
        gate: Arc<TurnGate>,
        log: Arc<RequestLog>,
    }

    #[async_trait::async_trait]
    impl LlmClient for CompletionClient {
        fn project_replay_messages(&self, messages: &[Message]) -> Result<Vec<Message>, LlmError> {
            Ok(messages.to_vec())
        }

        fn stream<'a>(&'a self, request: &'a LlmRequest) -> LlmStream<'a> {
            self.log.record(request);
            let users = request
                .messages
                .iter()
                .filter_map(|message| match message {
                    Message::User(user) => Some(user.text_content()),
                    _ => None,
                })
                .collect::<Vec<_>>();
            let all_users = users.join("\n");
            let role = all_users
                .rsplit_once("You are '")
                .and_then(|(_, rest)| rest.split_once('\''))
                .map(|(role, _)| role);
            let (held, text) = if all_users.contains("bounded plain-text summary") {
                (false, COUNCIL_SUMMARY.to_string())
            } else if users.last().is_some_and(|text| text.contains(CHILD_TASK)) {
                (true, CHILD_REPLY.to_string())
            } else if let Some(role) = role {
                (true, format!("position from {role}"))
            } else {
                (false, "owner acknowledged".to_string())
            };
            let gate = Arc::clone(&self.gate);
            let model = request.model.clone();
            Box::pin(
                futures::stream::once(async move {
                    if held {
                        gate.wait().await;
                    }
                    vec![
                        LlmEvent::TextDelta {
                            delta: text,
                            meta: None,
                        },
                        LlmEvent::UsageUpdate {
                            usage: meerkat_core::TurnUsage::host_declared(
                                meerkat_core::Provider::OpenAI,
                                &model,
                                meerkat_core::Usage::default(),
                            ),
                        },
                        LlmEvent::Done {
                            outcome: meerkat_client::LlmDoneOutcome::success(
                                meerkat_core::StopReason::EndTurn,
                            ),
                        },
                    ]
                })
                .flat_map(|events| futures::stream::iter(events.into_iter().map(Ok))),
            )
        }

        fn provider(&self) -> meerkat_core::Provider {
            meerkat_core::Provider::OpenAI
        }

        async fn health_check(&self) -> Result<(), LlmError> {
            Ok(())
        }
    }

    struct CompletionFixture {
        runtime: MobRuntime,
        machine: Arc<meerkat_runtime::MeerkatMachine>,
        service: Arc<PersistentSessionService<FactoryAgentBuilder>>,
        runtime_store: Arc<dyn meerkat_runtime::RuntimeStore>,
        jobs: Arc<dyn meerkat::DetachedJobStore>,
        delivery: MobRuntimeDelivery,
        log: Arc<RequestLog>,
        _temp: tempfile::TempDir,
    }

    impl CompletionFixture {
        async fn start(gate: Arc<TurnGate>, members: &[&str]) -> Self {
            let temp = tempfile::tempdir().expect("completion fixture directory");
            let session_store: Arc<dyn meerkat::SessionStore> =
                Arc::new(meerkat::MemoryStore::new());
            let runtime_store: Arc<dyn meerkat_runtime::RuntimeStore> =
                Arc::new(meerkat_runtime::InMemoryRuntimeStore::new());
            let blob_store: Arc<dyn meerkat_core::BlobStore> =
                Arc::new(meerkat_store::MemoryBlobStore::new());
            let jobs: Arc<dyn meerkat::DetachedJobStore> =
                Arc::new(meerkat::MemoryDetachedJobStore::new());
            let delivery = MobRuntimeDelivery::new(Arc::clone(&runtime_store), Arc::clone(&jobs));
            let log = Arc::new(RequestLog::default());
            let client: Arc<dyn LlmClient> = Arc::new(CompletionClient {
                gate,
                log: Arc::clone(&log),
            });
            let factory = meerkat::AgentFactory::new(temp.path()).comms(true);
            let mut builder = FactoryAgentBuilder::new(factory, Config::default());
            builder.default_session_store = Some(Arc::new(meerkat_store::StoreAdapter::new(
                Arc::clone(&session_store),
            )));
            builder.default_blob_store = Some(Arc::clone(&blob_store));
            builder.default_detached_job_store = Some(Arc::clone(&jobs));
            builder.default_llm_client = Some(Arc::clone(&client));
            let slot = Arc::clone(&builder.default_mob_tools);
            let adapter = Arc::new(
                meerkat_runtime::MeerkatMachine::persistent(
                    Arc::clone(&runtime_store),
                    Arc::clone(&blob_store),
                )
                .expect("completion runtime machine"),
            );
            let service = Arc::new(PersistentSessionService::new(
                builder,
                16,
                session_store,
                Arc::clone(&runtime_store),
                blob_store,
            ));
            let spec = MobBootstrapSpec::new(
                definition(&format!("delivery-{}", uuid::Uuid::new_v4().simple())),
                meerkat_mob::MobStorage::in_memory(),
                service.clone(),
            )
            .with_session_runtime_adapter(Arc::clone(&adapter))
            .expect("acquire completion fixture owner")
            .with_runtime_delivery(delivery.clone())
            .with_agent_mob_tools(slot)
            .expect("install actual agent mob tools")
            .with_options(MobBootstrapOptions {
                allow_ephemeral_sessions: true,
                notify_orchestrator_on_resume: false,
                default_llm_client: Some(client),
            });
            let runtime = tokio::time::timeout(WAIT, Box::pin(MobRuntime::bootstrap(spec)))
                .await
                .expect("bounded completion fixture bootstrap")
                .expect("bootstrap completion fixture");
            for member in members {
                let mut spawn = SpawnMemberSpec::new("general", *member);
                spawn.runtime_mode = Some(meerkat_mob::MobRuntimeMode::TurnDriven);
                tokio::time::timeout(WAIT, runtime.handle().spawn_spec(spawn))
                    .await
                    .expect("bounded source member spawn")
                    .expect("seat source member");
            }
            assert!(
                delivery.is_running(),
                "MobRuntime retains its native delivery owner"
            );
            let fixture = Self {
                runtime,
                machine: adapter,
                service,
                runtime_store,
                jobs,
                delivery,
                log,
                _temp: temp,
            };
            fixture.assert_no_callback_jobs().await;
            fixture
        }

        async fn session(&self, member: &str) -> SessionId {
            tokio::time::timeout(
                WAIT,
                self.runtime
                    .handle()
                    .resolve_bridge_session_id(&AgentIdentity::from(member)),
            )
            .await
            .expect("bounded member session lookup")
            .expect("source member session")
        }

        async fn messages(&self, session: &SessionId) -> Vec<Message> {
            tokio::time::timeout(WAIT, self.service.load_persisted_session(session))
                .await
                .expect("bounded owner transcript read")
                .expect("load owner transcript")
                .expect("owner transcript exists")
                .messages()
                .to_vec()
        }

        async fn assert_no_callback_jobs(&self) {
            assert!(
                self.jobs
                    .list_all(1)
                    .await
                    .expect("read callback jobs")
                    .is_empty(),
                "fork and council completion must not depend on callback-origin enumeration"
            );
        }

        async fn surface(&self, member: &str, council: bool) -> Arc<dyn AgentToolDispatcher> {
            let handle = self.runtime.handle();
            let authority =
                meerkat_runtime::mob_operator_authority::create_only_mob_operator_authority()
                    .expect("native fixture authority");
            let authority = if council {
                meerkat_runtime::mob_operator_authority::grant_manage_mob(
                    &authority,
                    handle.mob_id().as_str(),
                    handle.live_incarnation(),
                )
                .expect("council may use the source mob")
            } else {
                let authority = meerkat_runtime::mob_operator_authority::set_create_authority(
                    &authority, false,
                )
                .expect("fork has no create scope");
                meerkat_runtime::mob_operator_authority::grant_spawn_profile_in_mob(
                    &authority,
                    handle.mob_id().as_str(),
                    handle.live_incarnation(),
                    "general",
                )
                .expect("fork may spawn only its source profile")
            };
            let session = self.session(member).await;
            let surface: Arc<dyn AgentToolDispatcher> =
                Arc::new(meerkat_mob_mcp::AgentMobToolSurface::new(
                    self.runtime.agent_mob_mcp_state().expect("final MCP state"),
                    None,
                    authority,
                    "gpt-5.5".to_string(),
                    session.clone(),
                    None,
                    None,
                    None,
                ));
            surface
                .bind_ops_lifecycle(
                    Arc::new(meerkat_runtime::ops_lifecycle::RuntimeOpsLifecycleRegistry::new()),
                    session,
                )
                .expect("bind native agent tool lifecycle")
                .into_dispatcher()
        }

        async fn member_address(&self, member: &str) -> meerkat_runtime::LogicalRuntimeId {
            let handle = self.runtime.handle();
            let identity = AgentIdentity::from(member);
            let generation = handle
                .roster()
                .await
                .get_by_identity(&identity)
                .expect("seated completion owner")
                .generation
                .get();
            meerkat::member_delivery_address(
                handle.mob_id().as_str(),
                handle.incarnation(),
                member,
                generation,
            )
            .expect("native member address")
        }

        async fn wait_for_council_participant(
            &self,
            gate: &TurnGate,
            council_id: &str,
            job_id: &str,
        ) {
            let council_id = meerkat_mob::temporary_council::TemporaryCouncilId::new(council_id)
                .expect("returned council identity");
            let store = self
                .runtime
                .agent_mob_mcp_state()
                .expect("final MCP state")
                .temporary_council_store_for_tests();
            let mut entered = gate.entered.subscribe();
            let mut progress = None;
            let reached = tokio::time::timeout(WAIT, async {
                loop {
                    if *entered.borrow_and_update() > 0 {
                        return;
                    }
                    if let Some(record) =
                        store.load(&council_id).await.expect("read council custody")
                    {
                        assert_eq!(
                            record.detached_job.as_ref().map(|job| job.job_id.as_str()),
                            Some(job_id),
                            "observe only this detached council's custody"
                        );
                        // Project only typed progress, never the record's bearer capabilities.
                        progress = Some((
                            record.machine_state.lifecycle_phase,
                            record
                                .participants
                                .iter()
                                .map(|participant| {
                                    (
                                        participant.order,
                                        participant.acquisition,
                                        participant.seated,
                                    )
                                })
                                .collect::<Vec<_>>(),
                        ));
                        if let Some(result) = record.result {
                            panic!(
                                "council sealed before its first provider gate: {:?}",
                                result.exit_reason,
                            );
                        }
                    }
                    tokio::select! {
                        changed = entered.changed() => {
                            changed.expect("gate remains owned");
                        },
                        () = tokio::time::sleep(Duration::from_millis(25)) => {},
                    }
                }
            })
            .await;
            assert!(
                reached.is_ok(),
                "council provider gate timed out; last typed custody progress: {progress:?}",
            );
        }

        async fn assert_no_completion(&self, member: &str, job_id: &str) {
            let address = self.member_address(member).await;
            let session = self.session(member).await;
            assert!(completion_records(&self.messages(&session).await, job_id).is_empty());
            assert!(
                self.runtime_store
                    .list_runtime_delivery_records(&address, 0, 1)
                    .await
                    .expect("read held completion's member inbox")
                    .is_empty(),
                "a held child has not submitted any completion"
            );
            self.assert_no_callback_jobs().await;
        }

        async fn assert_delivered(&self, member: &str, job_id: &str, expected: &str) -> Value {
            let handle = self.runtime.handle();
            let identity = AgentIdentity::from(member);
            let address = self.member_address(member).await;
            let session = self.session(member).await;
            let inbox = self.delivery.inbox();
            tokio::time::timeout(WAIT, async {
                loop {
                    let rows = self
                        .runtime_store
                        .list_runtime_delivery_records(&address, 0, 2)
                        .await
                        .expect("read native member inbox");
                    assert!(rows.len() <= 1, "one continuation row for one completion");
                    if let Some(row) = rows.first() {
                        let delivery_id = meerkat_runtime::RuntimeDeliveryId::new(row.delivery_id())
                            .expect("native delivery id");
                        let status = inbox
                            .delivery_status(&address, &delivery_id)
                            .await
                            .expect("native delivery status");
                        if matches!(status, meerkat_runtime::RuntimeDeliveryStatus::Applied { .. }) {
                            let admission = inbox
                                .continuation_admission(&address, &delivery_id)
                                .await
                                .expect("native admission receipt");
                            assert!(
                                matches!(
                                    admission,
                                    Some(meerkat_runtime::ContinuationAdmission::Applied { session_id, .. })
                                        if session_id == session
                                ),
                                "the member inbox was applied to the exact owner session"
                            );
                            break;
                        }
                    }
                    tokio::time::sleep(Duration::from_millis(25)).await;
                }
            })
            .await
            .expect("native owner applies the member-addressed continuation");
            self.log.wait_for_completion_request(job_id).await;
            assert_eq!(
                self.log.requests_with_completion(job_id),
                1,
                "one completion wake"
            );

            let bounded = meerkat_mob::BoundedResultSpec::new("delivery-follow-up", 1024)
                .expect("bounded follow-up");
            let work = tokio::time::timeout(
                WAIT,
                handle.start_work_for_identity_bounded(
                    identity,
                    meerkat_mob::WorkSpec::new(FOLLOW_UP, meerkat_mob::WorkOrigin::Internal),
                    meerkat_core::HandlingMode::Queue,
                    bounded.clone(),
                ),
            )
            .await
            .expect("bounded follow-up admission")
            .expect("queue owner follow-up");
            let result = tokio::time::timeout(WAIT, work.wait_bounded(bounded))
                .await
                .expect("bounded owner follow-up")
                .expect("owner follow-up succeeds");
            assert_eq!(result.result().result().text(), "owner acknowledged");
            self.log.assert_follow_up_retains_completion(job_id);
            assert_eq!(
                self.log.requests_with_completion(job_id),
                2,
                "one wake and one explicit follow-up carry the same completion"
            );
            let records = completion_records(&self.messages(&session).await, job_id);
            assert_eq!(
                records.len(),
                1,
                "one persisted completion after the follow-up"
            );
            assert_eq!(records[0].0, BackgroundJobTerminalStatus::Completed);
            assert!(
                records[0].1.contains(expected),
                "completion retains the child result"
            );
            self.assert_no_callback_jobs().await;
            serde_json::from_str(&records[0].1).expect("typed completion detail")
        }

        /// The latest pass of the delivery owner once it satisfies `done`.
        async fn wait_for_pass(
            &self,
            what: &str,
            done: impl Fn(&meerkat::RuntimeDeliveryPass) -> bool,
        ) -> meerkat::RuntimeDeliveryPass {
            let reached = tokio::time::timeout(WAIT, async {
                loop {
                    if let Some(pass) = self.delivery.last_pass()
                        && done(&pass)
                    {
                        break pass;
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await;
            reached.unwrap_or_else(|_| panic!("{what}: last pass {:?}", self.delivery.last_pass()))
        }

        /// An acknowledged cutpoint of the delivery owner's loop. Twice, it
        /// reads the latest published pass, commits a row the host's resolver
        /// strands (for a member of a mob this host does not manage, so it
        /// starts no session activity) and waits for the next pass. The second
        /// of these passes began after the pass running when this was called
        /// (if any) had ended, and the loop captures every signal before the
        /// pass it runs, so each signal published before this call has been
        /// consumed, and acted on if it had work, when this returns.
        async fn cutpoint(&self) -> meerkat::RuntimeDeliveryPass {
            self.pass_after_a_stranded_row().await;
            self.pass_after_a_stranded_row().await
        }

        async fn pass_after_a_stranded_row(&self) -> meerkat::RuntimeDeliveryPass {
            let published = self.delivery.last_pass().map_or(0, |pass| pass.generation);
            let mob_id = format!("foreign-{}", uuid::Uuid::new_v4().simple());
            let address = meerkat::member_delivery_address(
                &mob_id,
                meerkat_core::service::MobIncarnationId::mint(),
                "absent",
                1,
            )
            .expect("foreign member address");
            let owner = meerkat::ContinuationOwner::Member {
                mob_id: mob_id.clone(),
                identity: "absent".to_string(),
            };
            meerkat::ContinuationOwnerService::new(
                self.delivery.inbox(),
                Arc::new(ForeignMember(address)),
                Arc::clone(&self.machine),
            )
            .submit(
                &owner,
                continuation(&format!("cutpoint-{mob_id}"), "cutpoint"),
                1,
            )
            .await
            .expect("commit the cutpoint row");
            self.wait_for_pass("the pass after the cutpoint row", |pass| {
                pass.generation > published
            })
            .await
        }

        /// A continuation owed to `member`'s session as a plain-session owner,
        /// submitted through the bound state's continuation service while the
        /// host's owner custody is not restored: the host's resolver asks the
        /// owner hook, which refuses, so the delivery owner holds the row
        /// blocked.
        async fn blocked_continuation(&self, member: &str, body: &str) -> BlockedContinuation {
            let state = self.runtime.agent_mob_mcp_state().expect("final MCP state");
            let custody = OwnerCustody::new();
            let hook: Arc<dyn meerkat_mob_mcp::DetachedOwnerHost> = custody.clone();
            state.set_detached_owner_host(Some(hook));
            let session = self.session(member).await;
            let owner = meerkat::ContinuationOwner::Session {
                session_id: session.clone(),
            };
            let continuations = state
                .continuation_service()
                .expect("the bound continuation service");
            let delivery = continuation(&format!("wake-{}", uuid::Uuid::new_v4().simple()), body);
            let key = delivery.key.clone();
            let receipt = continuations
                .submit(&owner, delivery, 1)
                .await
                .expect("commit the continuation");
            let address = meerkat_runtime::LogicalRuntimeId::new(receipt.address.clone());
            let delivery_id = meerkat_runtime::RuntimeDeliveryId::new(receipt.delivery_id)
                .expect("native delivery id");
            self.wait_for_pass("the owner holds the continuation blocked", |pass| {
                pass.blocked_runtimes.contains(&address)
            })
            .await;
            assert!(
                custody.asks() >= 1 && custody.asked_only(&session),
                "the host's resolver asked the owner hook about this session"
            );
            let blocked = BlockedContinuation {
                custody,
                session,
                owner,
                key,
                address,
                delivery_id,
                continuations,
                inbox: self.delivery.inbox(),
            };
            blocked.assert_not_admitted().await;
            blocked
        }

        async fn stop(self) {
            let report = tokio::time::timeout(WAIT, self.runtime.handle().destroy())
                .await
                .expect("bounded fixture destruction")
                .expect("destroy fixture mob");
            assert!(report.errors.is_empty());
            assert!(report.orphaned_remote_members.is_empty());
            assert!(!report.remote_cleanup_deadline_exceeded);
            assert!(report.metadata_scrubbed && report.events_cleared && report.namespace_cleaned);
        }
    }

    /// A host continuation, queued for its owner.
    fn continuation(key: &str, body: &str) -> meerkat::ContinuationDelivery {
        meerkat::ContinuationDelivery {
            key: meerkat::ContinuationKey::new(key).expect("continuation key"),
            result: meerkat::ContinuationResultRef {
                producer: meerkat::ContinuationProducer::Host {
                    namespace: "meta-wake".to_string(),
                },
                producer_id: key.to_string(),
                result_digest: "sha256:meta-wake".to_string(),
                summary: None,
            },
            body: body.into(),
            handling: meerkat::ContinuationHandling::Queue,
        }
    }

    /// Only a foreign member's address: a member of a mob this host does not
    /// manage. Once the host's mob set is restored its resolver strands such
    /// rows: never applied, never blocked.
    struct ForeignMember(meerkat_runtime::LogicalRuntimeId);

    #[async_trait::async_trait]
    impl meerkat::ContinuationAddressResolver for ForeignMember {
        async fn current_address(
            &self,
            _owner: &meerkat::ContinuationOwner,
        ) -> Result<Option<meerkat_runtime::LogicalRuntimeId>, String> {
            Ok(Some(self.0.clone()))
        }

        async fn resolve_address(
            &self,
            _address: &meerkat_runtime::LogicalRuntimeId,
        ) -> Result<meerkat::AddressResolution, String> {
            Ok(meerkat::AddressResolution::NotServed)
        }
    }

    /// The host's owner hook for plain-session owners, standing for custody
    /// the host restores later: until then it cannot make the owner live
    /// (`Failed`), so the host's resolver leaves that owner's rows unserved.
    /// Restoring it raises no signal. Every ask is recorded and acknowledged.
    struct OwnerCustody {
        restored: AtomicBool,
        asked: Mutex<Vec<SessionId>>,
        asks: tokio::sync::watch::Sender<usize>,
    }

    impl OwnerCustody {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                restored: AtomicBool::new(false),
                asked: Mutex::new(Vec::new()),
                asks: tokio::sync::watch::Sender::new(0),
            })
        }

        /// Restore the custody silently: nothing is announced.
        fn restore(&self) {
            self.restored.store(true, Ordering::SeqCst);
        }

        fn asks(&self) -> usize {
            *self.asks.borrow()
        }

        fn asked_only(&self, session: &SessionId) -> bool {
            self.asked
                .lock()
                .expect("owner hook asks")
                .iter()
                .all(|asked| asked == session)
        }

        async fn wait_for_asks(&self, count: usize) {
            let mut asks = self.asks.subscribe();
            tokio::time::timeout(WAIT, asks.wait_for(|seen| *seen >= count))
                .await
                .expect("the delivery owner asks the owner hook again")
                .expect("owner hook remains owned");
        }
    }

    #[async_trait::async_trait]
    impl meerkat_mob_mcp::DetachedOwnerHost for OwnerCustody {
        async fn ensure_owner_live(
            &self,
            session_id: &SessionId,
        ) -> Result<(), meerkat_mob_mcp::DetachedOwnerError> {
            self.asked
                .lock()
                .expect("owner hook asks")
                .push(session_id.clone());
            let restored = self.restored.load(Ordering::SeqCst);
            self.asks.send_modify(|seen| *seen += 1);
            if restored {
                Ok(())
            } else {
                Err(meerkat_mob_mcp::DetachedOwnerError::Failed {
                    detail: "the owner's custody is not restored".to_string(),
                })
            }
        }
    }

    /// One continuation the delivery owner holds blocked on the host's owner
    /// custody.
    struct BlockedContinuation {
        custody: Arc<OwnerCustody>,
        session: SessionId,
        owner: meerkat::ContinuationOwner,
        key: meerkat::ContinuationKey,
        address: meerkat_runtime::LogicalRuntimeId,
        delivery_id: meerkat_runtime::RuntimeDeliveryId,
        continuations: Arc<meerkat::ContinuationOwnerService>,
        inbox: meerkat_runtime::RuntimeDeliveryInbox,
    }

    impl BlockedContinuation {
        /// The owner service's status, read only once the row is applied (an
        /// applied row's status asks no resolver).
        async fn status(&self) -> meerkat::ContinuationStatus {
            self.continuations
                .continuation_status(&self.owner, &self.key)
                .await
                .expect("continuation status")
        }

        /// Pending, and no session admitted it. Read from the inbox: the
        /// owner service's status read resolves the address of a row not yet
        /// admitted, which would ask the owner hook itself.
        async fn assert_not_admitted(&self) {
            let status = self
                .inbox
                .delivery_status(&self.address, &self.delivery_id)
                .await
                .expect("native delivery status");
            assert!(
                matches!(
                    status,
                    meerkat_runtime::RuntimeDeliveryStatus::Pending { .. }
                ),
                "still pending: {status:?}"
            );
            assert!(
                self.inbox
                    .continuation_admission(&self.address, &self.delivery_id)
                    .await
                    .expect("admission")
                    .is_none(),
                "no session admitted it"
            );
        }

        async fn wait_until_applied(&self) {
            tokio::time::timeout(WAIT, async {
                loop {
                    if matches!(
                        self.inbox
                            .delivery_status(&self.address, &self.delivery_id)
                            .await
                            .expect("native delivery status"),
                        meerkat_runtime::RuntimeDeliveryStatus::Applied { .. }
                    ) {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .expect("the announced retry applies the continuation");
        }

        /// Applied once, to exactly its owner's session.
        async fn assert_applied_to_its_session(&self) -> meerkat::ContinuationStatus {
            assert!(
                matches!(
                    self.inbox
                        .continuation_admission(&self.address, &self.delivery_id)
                        .await
                        .expect("admission"),
                    Some(meerkat_runtime::ContinuationAdmission::Applied { session_id, .. })
                        if session_id == self.session
                ),
                "admitted into the exact owner session"
            );
            let status = self.status().await;
            assert!(
                matches!(
                    &status,
                    meerkat::ContinuationStatus::Applied { session, .. } if session == &self.session
                ),
                "{status:?}"
            );
            status
        }
    }

    /// Wait for the announcer's own `false`: the last holder of the bindings
    /// is gone. The stopped owner's task drops its host's clone of them when
    /// it ends; once no strong holder is left, `false` is final.
    async fn bindings_disposed(announcer: &ContinuationSourceAnnouncer) {
        tokio::time::timeout(WAIT, async {
            while announcer.announce_source_custody_changed() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("final disposal leaves no holder of the bindings");
    }

    fn completion_records(
        messages: &[Message],
        job_id: &str,
    ) -> Vec<(BackgroundJobTerminalStatus, String)> {
        messages
            .iter()
            .filter_map(|message| match message {
                Message::SystemNotice(notice) if notice.kind == SystemNoticeKind::BackgroundJob => {
                    notice.blocks.iter().find_map(|block| match block {
                        SystemNoticeBlock::BackgroundJob {
                            job_id: id,
                            status,
                            detail,
                            persisted: true,
                            ..
                        } if id == job_id => Some((*status, detail.clone().unwrap_or_default())),
                        _ => None,
                    })
                }
                _ => None,
            })
            .collect()
    }

    async fn start_detached(
        surface: &Arc<dyn AgentToolDispatcher>,
        tool: &str,
        args: Value,
    ) -> Value {
        let raw =
            serde_json::value::RawValue::from_string(args.to_string()).expect("tool arguments");
        let outcome = tokio::time::timeout(
            WAIT,
            surface.dispatch(ToolCallView {
                id: "detached-completion-fixture",
                name: tool,
                args: &raw,
            }),
        )
        .await
        .expect("tool returns before its held child completes")
        .expect("native agent tool starts");
        assert!(
            !outcome.result.is_error,
            "{}",
            outcome.result.text_content()
        );
        let started: Value =
            serde_json::from_str(&outcome.result.text_content()).expect("tool result");
        assert_eq!(started["status"], "running", "{started}");
        assert!(started["job_id"].as_str().is_some_and(|id| !id.is_empty()));
        assert!(started.get("blocked_because").is_none(), "{started}");
        started
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn fork_completion_reaches_member_with_no_callback_jobs() {
        let gate = TurnGate::new();
        let _release = ReleaseOnDrop(Arc::clone(&gate));
        let fixture = CompletionFixture::start(Arc::clone(&gate), &["owner"]).await;
        let surface = fixture.surface("owner", false).await;
        let started = start_detached(
            &surface,
            "fork_off",
            json!({
                "member_id": "child", "task": CHILD_TASK, "max_run_secs": 120,
            }),
        )
        .await;
        let job_id = started["job_id"].as_str().expect("fork job id");
        gate.wait_entered(1).await;
        fixture.assert_no_completion("owner", job_id).await;
        assert_eq!(
            fixture
                .runtime
                .handle()
                .get_member(&AgentIdentity::from("child"))
                .await
                .expect("fork child")
                .expect("child remains seated")
                .spawned_by,
            Some(AgentIdentity::from("owner"))
        );
        gate.release();
        fixture.assert_delivered("owner", job_id, CHILD_REPLY).await;
        fixture.stop().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn council_completion_reaches_member_with_no_callback_jobs() {
        let gate = TurnGate::new();
        let _release = ReleaseOnDrop(Arc::clone(&gate));
        let fixture =
            CompletionFixture::start(Arc::clone(&gate), &["convener", "alice", "bob"]).await;
        let surface = fixture.surface("convener", true).await;
        let mob_id = fixture.runtime.handle().mob_id().to_string();
        let started = start_detached(
            &surface,
            "council",
            json!({
                "topic": "Should we ship the delivery repair?",
                "participants": [
                    {"mob_id": mob_id, "member_id": "alice", "role": "analyst"},
                    {"mob_id": mob_id, "member_id": "bob", "role": "critic"},
                ],
                "max_rounds": 1, "timeout_seconds": 120,
            }),
        )
        .await;
        let council_id = started["council_id"].as_str().expect("council identity");
        let job_id = started["job_id"].as_str().expect("council job id");
        fixture
            .wait_for_council_participant(&gate, council_id, job_id)
            .await;
        fixture.assert_no_completion("convener", job_id).await;
        gate.release();
        let outcome = fixture
            .assert_delivered("convener", job_id, COUNCIL_SUMMARY)
            .await;
        assert_eq!(
            outcome["result"]["exit_reason"]["reason"], "completed",
            "{outcome}"
        );
        let participants = outcome["result"]["participants"]
            .as_array()
            .expect("council participants");
        assert_eq!(participants.len(), 2);
        assert!(
            participants
                .iter()
                .all(|participant| participant["seated"] == true)
        );
        assert!(
            outcome["result"]["exchanges"]
                .as_array()
                .is_some_and(|exchanges| exchanges.len() >= 2)
        );
        fixture.stop().await;
    }

    /// A continuation blocked on the host's owner custody is not retried when
    /// the custody is restored silently: up to an acknowledged cutpoint
    /// nobody asked the owner hook again and nothing was admitted. The
    /// runtime's announcer then wakes Meta's own delivery owner, which asks
    /// once more and applies the continuation to its exact session once. A
    /// repeated announcement replays nothing.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_announced_custody_restore_resumes_a_blocked_continuation_once() {
        let gate = TurnGate::new();
        let _release = ReleaseOnDrop(Arc::clone(&gate));
        let fixture = CompletionFixture::start(Arc::clone(&gate), &["owner"]).await;
        let blocked = fixture.blocked_continuation("owner", ANNOUNCED).await;
        // Every signal raised so far has been acted on.
        fixture.cutpoint().await;

        let asked = blocked.custody.asks();
        blocked.custody.restore();
        let quiet = fixture.cutpoint().await;
        assert!(
            quiet.blocked_runtimes.contains(&blocked.address),
            "a silent restore retries nothing"
        );
        assert_eq!(blocked.custody.asks(), asked, "nobody asked the owner hook");
        blocked.assert_not_admitted().await;

        let announcer = fixture
            .runtime
            .continuation_source_announcer()
            .expect("the delivery composition supplies an announcer");
        assert!(announcer.announce_source_custody_changed());
        blocked.wait_until_applied().await;
        assert_eq!(blocked.custody.asks(), asked + 1, "one fresh resolution");
        assert!(blocked.custody.asked_only(&blocked.session));
        let applied = blocked.assert_applied_to_its_session().await;
        fixture.log.wait_for_turn_started_by(ANNOUNCED).await;
        assert_eq!(fixture.log.turns_started_by(ANNOUNCED), 1);

        assert!(announcer.announce_source_custody_changed());
        let after = fixture.cutpoint().await;
        assert!(!after.blocked_runtimes.contains(&blocked.address));
        assert_eq!(blocked.status().await, applied, "the same single admission");
        assert_eq!(
            blocked.custody.asks(),
            asked + 1,
            "an applied row is not resolved again"
        );
        assert_eq!(
            fixture.log.turns_started_by(ANNOUNCED),
            1,
            "nothing replayed into the session"
        );
        fixture.stop().await;
    }

    /// An announcement while the owner custody is still refused gives no
    /// admission: the delivery owner asks the owner hook again (acknowledged
    /// by the hook), the hook still refuses, and the continuation stays
    /// pending and blocked with no session admitting it.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_announcement_while_custody_is_refused_admits_nothing() {
        let gate = TurnGate::new();
        let _release = ReleaseOnDrop(Arc::clone(&gate));
        let fixture = CompletionFixture::start(Arc::clone(&gate), &["owner"]).await;
        let blocked = fixture.blocked_continuation("owner", ANNOUNCED).await;
        fixture.cutpoint().await;

        let asked = blocked.custody.asks();
        let announcer = fixture
            .runtime
            .continuation_source_announcer()
            .expect("the delivery composition supplies an announcer");
        assert!(announcer.announce_source_custody_changed());
        blocked.custody.wait_for_asks(asked + 1).await;
        let settled = fixture.cutpoint().await;
        assert!(
            settled.blocked_runtimes.contains(&blocked.address),
            "still blocked"
        );
        assert_eq!(blocked.custody.asks(), asked + 1, "asked exactly once more");
        blocked.assert_not_admitted().await;
        assert_eq!(fixture.log.turns_started_by(ANNOUNCED), 0);
        fixture.stop().await;
    }

    /// A stopped delivery owner is not final disposal. After an explicit stop
    /// the runtime still supplies its announcer, and while a setup clone of
    /// the delivery composition keeps the bindings alive the announcement
    /// reports `true`, yet nothing acts on it: with no owner over the inbox
    /// (this test holds its ownership) the continuation is not admitted even
    /// though its custody was restored. Once the runtime and every clone are
    /// disposed of, the announcer alone answers `false`.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_stopped_owner_is_not_disposal_and_only_disposal_ends_the_announcer() {
        let gate = TurnGate::new();
        let _release = ReleaseOnDrop(Arc::clone(&gate));
        let fixture = CompletionFixture::start(Arc::clone(&gate), &["owner"]).await;
        let blocked = fixture.blocked_continuation("owner", ANNOUNCED).await;
        fixture.cutpoint().await;
        let announcer = fixture
            .runtime
            .continuation_source_announcer()
            .expect("the delivery composition supplies an announcer");

        fixture.runtime.stop_runtime_delivery();
        let inbox = fixture.delivery.inbox();
        let claim = tokio::time::timeout(WAIT, async {
            loop {
                if let Ok(claim) = inbox.claim_delivery_ownership() {
                    break claim;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the stopped owner releases the inbox");
        assert!(
            fixture.runtime.continuation_source_announcer().is_some(),
            "the retained delivery survives the explicit stop"
        );
        blocked.custody.restore();
        assert!(
            announcer.announce_source_custody_changed(),
            "the setup clone keeps the bindings alive"
        );
        blocked.assert_not_admitted().await;
        assert_eq!(fixture.log.turns_started_by(ANNOUNCED), 0);
        drop(claim);

        fixture.stop().await;
        bindings_disposed(&announcer).await;
        assert!(
            !announcer.announce_source_custody_changed(),
            "the announcer alone keeps nothing alive"
        );
    }
}
