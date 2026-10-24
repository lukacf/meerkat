//! A host store writer whose self-fence window closes dispatches nothing more:
//! a real native agent runs over the store checkpointer of a host store realm,
//! and the window closes while one of its round commits lands. Unrun until the
//! candidate's validation.

use super::*;
use futures::stream;
use meerkat_client::{LlmDoneOutcome, LlmError, LlmEvent, LlmRequest};
use meerkat_core::host_store::{
    AcquireMode, CommitBatch, CommitId, FenceGrant, FenceTiming, FenceToken, HeadPage, HostStore,
    HostStoreError, Row, RowIndex, ScopeHead, ScopeKind, ScopeName, ScopeRef, ScopeVersion,
    VersionedHead,
};
use meerkat_core::service::{DeferredPromptPolicy, InitialTurnPolicy};
use meerkat_core::{
    AgentError, AgentToolDispatcher, Provider, ToolCallView, ToolDef, ToolDispatchOutcome,
    ToolError, ToolResult,
};
use meerkat_runtime::MeerkatMachine;
use meerkat_runtime::store::spi::{SpiRealm, SpiRealmConfig, SpiRuntimeStore};
use meerkat_session::PersistentSessionService;
use meerkat_store::{MemoryBlobStore, MemoryStore};
use serde_json::json;
use std::collections::{BTreeMap, HashSet};
use std::pin::Pin;
use std::sync::Mutex as StdMutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;
use tempfile::TempDir;

const DEADLINE: Duration = Duration::from_secs(10);

/// Which round commit the writer's window closes during.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ClosesDuring {
    /// The round's first commit: the window is closed before any call of
    /// the round is dispatched.
    FirstCommit,
    /// The round's second commit: the window is closed before the next
    /// model call.
    SecondCommit,
}

/// Shared between the host, the model and the tools of one row.
#[derive(Default)]
struct FenceControl {
    /// Every renew from now on fails.
    fail_renews: AtomicBool,
    /// A renew failed since `fail_renews` was set.
    renew_failed: AtomicBool,
    /// The next host commit waits until a renew has failed, then lands.
    hold_next_commit: AtomicBool,
}

impl FenceControl {
    /// Close the writer's window while the next host commit is in flight:
    /// the commit has passed the write path's own fence check when the host
    /// sees it, and the renews fail only from then on, so the commit lands
    /// and the window is closed by the time it answers.
    fn close_during_next_commit(&self) {
        self.hold_next_commit.store(true, Ordering::SeqCst);
    }
}

#[derive(Debug, Clone)]
struct StoredScope {
    version: ScopeVersion,
    commit_id: CommitId,
    head: Vec<u8>,
    start: u64,
    rows: BTreeMap<u64, Vec<u8>>,
    end: u64,
}

#[derive(Default)]
struct HostState {
    token: u64,
    scopes: BTreeMap<(ScopeKind, String), StoredScope>,
    landed: HashSet<String>,
}

/// An in-memory host store that meets its obligations at commit, with a
/// 1 s renew interval and a 3 s lease, whose renews and commits follow
/// `control`.
struct FenceHost {
    state: StdMutex<HostState>,
    control: Arc<FenceControl>,
}

impl FenceHost {
    fn new(control: Arc<FenceControl>) -> Self {
        Self {
            state: StdMutex::new(HostState::default()),
            control,
        }
    }

    fn state(&self) -> std::sync::MutexGuard<'_, HostState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

#[async_trait]
impl HostStore for FenceHost {
    async fn acquire_fence(&self, _mode: AcquireMode) -> Result<FenceGrant, HostStoreError> {
        let mut state = self.state();
        state.token += 1;
        Ok(FenceGrant {
            token: FenceToken::new(state.token),
            superseded_lease: Duration::ZERO,
        })
    }

    async fn renew_fence(&self, token: FenceToken) -> Result<(), HostStoreError> {
        if self.control.fail_renews.load(Ordering::SeqCst) {
            self.control.renew_failed.store(true, Ordering::SeqCst);
            return Err(HostStoreError::Unavailable(
                "the row stops the renews".to_string(),
            ));
        }
        let state = self.state();
        if token.get() != state.token {
            return Err(HostStoreError::FencedOut {
                current: FenceToken::new(state.token),
            });
        }
        Ok(())
    }

    fn fence_timing(&self) -> FenceTiming {
        FenceTiming::try_new(Duration::from_secs(1), Duration::from_secs(3))
            .expect("a 1 s renew interval on a 3 s lease")
    }

    async fn load_head(&self, scope: &ScopeRef) -> Result<Option<VersionedHead>, HostStoreError> {
        Ok(self
            .state()
            .scopes
            .get(&(scope.kind, scope.name.as_str().to_string()))
            .map(|stored| VersionedHead {
                version: stored.version,
                commit_id: stored.commit_id.clone(),
                head: stored.head.clone(),
            }))
    }

    async fn list_heads(
        &self,
        kind: ScopeKind,
        after: Option<&ScopeName>,
        limit: usize,
    ) -> Result<HeadPage, HostStoreError> {
        let state = self.state();
        let mut heads = Vec::new();
        let mut more = false;
        for ((scope_kind, name), stored) in &state.scopes {
            if *scope_kind != kind || after.is_some_and(|after| name.as_str() <= after.as_str()) {
                continue;
            }
            if heads.len() == limit {
                more = true;
                break;
            }
            heads.push(ScopeHead {
                name: ScopeName::try_new(name.clone())?,
                head: VersionedHead {
                    version: stored.version,
                    commit_id: stored.commit_id.clone(),
                    head: stored.head.clone(),
                },
            });
        }
        let next = if more {
            heads.last().map(|head| head.name.clone())
        } else {
            None
        };
        Ok(HeadPage { heads, next })
    }

    async fn read_rows(
        &self,
        scope: &ScopeRef,
        from: RowIndex,
        limit: usize,
    ) -> Result<Vec<Row>, HostStoreError> {
        let state = self.state();
        let Some(stored) = state
            .scopes
            .get(&(scope.kind, scope.name.as_str().to_string()))
        else {
            return Ok(Vec::new());
        };
        stored
            .rows
            .range(from.get().max(stored.start)..)
            .take(limit)
            .map(|(_, row)| Row::try_new(row.clone()))
            .collect()
    }

    async fn commit(&self, batch: &CommitBatch) -> Result<(), HostStoreError> {
        if self.control.hold_next_commit.swap(false, Ordering::SeqCst) {
            // The writer passed its own fence check before this commit; its
            // window closes while the commit is in flight.
            self.control.fail_renews.store(true, Ordering::SeqCst);
            let _ = tokio::time::timeout(Duration::from_secs(5), async {
                while !self.control.renew_failed.load(Ordering::SeqCst) {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await;
        }
        let mut state = self.state();
        if state.landed.contains(batch.commit_id().as_str()) {
            return Ok(());
        }
        if batch.fence().get() < state.token {
            return Err(HostStoreError::FencedOut {
                current: FenceToken::new(state.token),
            });
        }
        for update in batch.updates() {
            let key = (update.scope.kind, update.scope.name.as_str().to_string());
            let current = state.scopes.get(&key).map(|stored| stored.version);
            if current != update.previous {
                return Err(HostStoreError::Conflict {
                    scope: update.scope.clone(),
                    current,
                });
            }
        }
        for update in batch.updates() {
            let key = (update.scope.kind, update.scope.name.as_str().to_string());
            if update.delete {
                state.scopes.remove(&key);
                continue;
            }
            let stored = state.scopes.entry(key).or_insert_with(|| StoredScope {
                version: update.next,
                commit_id: batch.commit_id().clone(),
                head: Vec::new(),
                start: 0,
                rows: BTreeMap::new(),
                end: 0,
            });
            if let Some(truncate) = update.truncate_after {
                stored.rows.retain(|index, _| *index < truncate.get());
                stored.end = stored.end.min(truncate.get());
            }
            if let Some(drop_before) = update.drop_before {
                stored.rows.retain(|index, _| *index >= drop_before.get());
                stored.start = stored.start.max(drop_before.get());
            }
            for row in &update.append {
                stored.rows.insert(stored.end, row.as_bytes().to_vec());
                stored.end += 1;
            }
            if let Some(head) = &update.head {
                stored.head = head.clone();
            }
            stored.version = update.next;
            stored.commit_id = batch.commit_id().clone();
        }
        state.landed.insert(batch.commit_id().as_str().to_string());
        Ok(())
    }
}

/// Asks for one call of `probe`, then answers with text. Counts its
/// requests.
struct FenceModel {
    requests: AtomicUsize,
    control: Arc<FenceControl>,
    closes_during: ClosesDuring,
}

#[async_trait]
impl LlmClient for FenceModel {
    fn project_replay_messages(
        &self,
        messages: &[meerkat_core::Message],
    ) -> Result<Vec<meerkat_core::Message>, LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(
        &'a self,
        request: &'a LlmRequest,
    ) -> Pin<Box<dyn futures::Stream<Item = Result<LlmEvent, LlmError>> + Send + 'a>> {
        let first = self.requests.fetch_add(1, Ordering::SeqCst) == 0;
        let usage = meerkat_core::TurnUsage::host_declared(
            Provider::Anthropic,
            &request.model,
            Default::default(),
        );
        if first {
            if self.closes_during == ClosesDuring::FirstCommit {
                self.control.close_during_next_commit();
            }
            return Box::pin(stream::iter(vec![
                Ok(LlmEvent::ToolCallComplete {
                    id: "probe-call".into(),
                    name: "probe".into(),
                    args: json!({}),
                    meta: None,
                }),
                Ok(LlmEvent::UsageUpdate { usage }),
                Ok(LlmEvent::Done {
                    outcome: LlmDoneOutcome::success(meerkat_core::StopReason::ToolUse),
                }),
            ]));
        }
        Box::pin(stream::iter(vec![
            Ok(LlmEvent::TextDelta {
                delta: "probed".into(),
                meta: None,
            }),
            Ok(LlmEvent::UsageUpdate { usage }),
            Ok(LlmEvent::Done {
                outcome: LlmDoneOutcome::success(meerkat_core::StopReason::EndTurn),
            }),
        ]))
    }

    fn provider(&self) -> Provider {
        Provider::Anthropic
    }

    async fn health_check(&self) -> Result<(), LlmError> {
        Ok(())
    }
}

/// One tool, `probe`. Counts its dispatches.
struct FenceTools {
    dispatched: AtomicUsize,
    control: Arc<FenceControl>,
    closes_during: ClosesDuring,
}

#[async_trait]
impl AgentToolDispatcher for FenceTools {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        vec![Arc::new(ToolDef::new(
            "probe",
            "a probe",
            json!({"type":"object"}),
        ))]
        .into()
    }

    async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
        self.dispatched.fetch_add(1, Ordering::SeqCst);
        if self.closes_during == ClosesDuring::SecondCommit {
            self.control.close_during_next_commit();
        }
        Ok(ToolDispatchOutcome::sync_result(ToolResult::new(
            call.id.into(),
            "probed".into(),
            false,
        )))
    }
}

/// A real native agent through the factory, with the row's model and tools.
struct FenceBuilder {
    factory: AgentFactory,
    client: Arc<FenceModel>,
    tools: Arc<FenceTools>,
}

#[async_trait]
impl SessionAgentBuilder for FenceBuilder {
    type Agent = FactoryAgent;

    async fn build_agent(
        &self,
        request: &CreateSessionRequest,
        event_tx: mpsc::Sender<AgentEvent>,
    ) -> Result<FactoryAgent, SessionError> {
        let mut config = AgentBuildConfig::from_create_session_request(request, event_tx);
        config.llm_client_override = Some(self.client.clone());
        config.tool_dispatcher_override = Some(self.tools.clone());
        let session_context = match request
            .build
            .as_ref()
            .map(|build| &build.runtime_build_mode)
        {
            Some(meerkat_core::RuntimeBuildMode::SessionOwned(bindings)) => {
                Some(Arc::clone(bindings.session_context()))
            }
            _ => None,
        };
        let factory = self.factory.clone();
        let agent = meerkat_runtime::stack_relief::relieve_caller_stack(move || async move {
            factory.build_agent(config, &Config::default()).await
        })
        .await
        .map_err(|error| SessionError::Agent(AgentError::InternalError(error.to_string())))?;
        Ok(FactoryAgent {
            agent,
            session_context,
            pending_head_canonical_boundary: None,
            acknowledged_head_canonical_boundary: None,
        })
    }
}

/// What a run whose window closed during one of its round commits did.
struct FencedRun {
    failed: bool,
    error: String,
    model_requests: usize,
    dispatched: usize,
    lease_live_after: bool,
}

/// Run one turn of a fresh session over a host store realm whose writer's
/// window closes during the round commit `closes_during` names.
async fn run_with_window_closing_during(closes_during: ClosesDuring) -> FencedRun {
    let temp = TempDir::new().expect("temp");
    let control = Arc::new(FenceControl::default());
    let host: Arc<dyn HostStore> = Arc::new(FenceHost::new(Arc::clone(&control)));
    let realm = SpiRealm::open(host, SpiRealmConfig::default())
        .await
        .expect("the realm opens");
    let runtime_store = Arc::new(
        SpiRuntimeStore::open(Arc::clone(&realm))
            .await
            .expect("the runtime store opens"),
    );
    let store = Arc::new(MemoryStore::new());
    let blob_store = Arc::new(MemoryBlobStore::new());
    let machine = Arc::new(
        MeerkatMachine::persistent(runtime_store.clone(), blob_store.clone())
            .expect("canonical machine"),
    );
    let client = Arc::new(FenceModel {
        requests: AtomicUsize::new(0),
        control: Arc::clone(&control),
        closes_during,
    });
    let tools = Arc::new(FenceTools {
        dispatched: AtomicUsize::new(0),
        control: Arc::clone(&control),
        closes_during,
    });
    let builder = FenceBuilder {
        factory: AgentFactory::new(temp.path().join("sessions")).session_store(store.clone()),
        client: client.clone(),
        tools: tools.clone(),
    };
    let service = Arc::new(
        PersistentSessionService::new(builder, 1, store.clone(), runtime_store, blob_store)
            .with_canonical_runtime_adapter(machine.clone()),
    );
    let request = CreateSessionRequest {
        model: "claude-sonnet-4-5".into(),
        prompt: "probe once".into(),
        injected_context: Vec::new(),
        system_prompt: meerkat_core::SystemPromptOverride::Inherit,
        max_tokens: None,
        event_tx: None,
        initial_turn: InitialTurnPolicy::RunImmediately,
        deferred_prompt_policy: DeferredPromptPolicy::Discard,
        build: None,
        labels: None,
    };
    let (request, initial_turn) =
        crate::surface::split_runtime_backed_eager_create_request(request);
    let created = Box::pin(crate::surface::materialize_session(
        &service,
        &machine,
        meerkat_core::Session::new(),
        request,
        {
            let service = service.clone();
            let machine = machine.clone();
            move |id| crate::surface::default_persistent_executor(service, machine, id)
        },
    ))
    .await
    .expect("materialize the session and its executor");
    let turn = tokio::time::timeout(
        DEADLINE,
        crate::surface::run_runtime_backed_initial_turn_with_machine(
            &service,
            &machine,
            &created.session_id,
            initial_turn.expect("initial turn"),
        ),
    )
    .await
    .expect("the turn ends");
    FencedRun {
        failed: turn.is_err(),
        error: turn
            .err()
            .map(|error| format!("{error:?}"))
            .unwrap_or_default(),
        model_requests: client.requests.load(Ordering::SeqCst),
        dispatched: tools.dispatched.load(Ordering::SeqCst),
        lease_live_after: realm.lease_is_live(),
    }
}

/// A host store writer whose self-fence window has closed makes no model
/// call and dispatches no tool, through the real store checkpointer. Its
/// round commit passes the write path's own fence check, and the window
/// closes while that commit is in flight, so only the dispatch fence stands
/// between the commit and the next step:
/// - closed during the round's first commit, the round's call is never
///   dispatched, and no model call follows;
/// - closed during the round's second commit, the call ran once, while the
///   window was open, and the next model call is refused.
#[tokio::test]
async fn a_writer_past_its_fence_window_makes_no_model_call_and_dispatches_no_tool() {
    let first = run_with_window_closing_during(ClosesDuring::FirstCommit).await;
    assert!(!first.lease_live_after, "the window closed");
    assert!(first.failed, "the run stops at the closed window");
    assert!(
        first.error.contains("self-fence window closed"),
        "the dispatch fence refused it, not the write path: {}",
        first.error
    );
    assert_eq!(first.dispatched, 0, "the round's call is never dispatched");
    assert_eq!(first.model_requests, 1, "no model call follows");

    let second = run_with_window_closing_during(ClosesDuring::SecondCommit).await;
    assert!(!second.lease_live_after, "the window closed");
    assert!(second.failed, "the run stops at the closed window");
    assert!(
        second.error.contains("self-fence window closed"),
        "the dispatch fence refused it, not the write path: {}",
        second.error
    );
    assert_eq!(
        second.dispatched, 1,
        "the call ran once, before the window closed"
    );
    assert_eq!(second.model_requests, 1, "the next model call is refused");
}
