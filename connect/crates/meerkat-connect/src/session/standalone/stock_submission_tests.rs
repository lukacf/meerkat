//! Consumer controls against the committed public native test-support contract.
//! The stage, finished bundle, controller ticket, session and pending original
//! are real. Terminal answers and loopback provider responses are fixture inputs.
//! These controls do not qualify provider credentials, MCP readiness or a product build.

use super::super::{RetainedSetupCloseError, StockSubmissionError};
use super::*;
use meerkat::governed_realm::stock::{StockGovernance, originals::StockCustodyError};
use meerkat_runtime::{
    AcceptOutcome, LogicalRuntimeId, RuntimeStore,
    completion::{CompletionHandle, CompletionOutcome},
    input_admission_custody::NativeInputAdmissionSettlement,
    input_state::InputTerminalOutcome,
};
use std::{
    collections::BTreeSet,
    os::unix::fs::PermissionsExt,
    sync::atomic::{AtomicUsize, Ordering},
};
use tokio::sync::{mpsc, oneshot};

const REALM: &str = "stock-submission";
const MODEL: &str = "claude-sonnet-4-5";
const ANSWER: &str = "The stock submission fixture completed";
#[path = "../../../tests/support/native_model_peer.rs"]
mod native_model_peer;
use native_model_peer::ModelPeer;

// These are observations of actual native HTTP request blocks, not native
// ToolResult holders or authority. Non-text fixture results are refused.
#[derive(Clone)]
struct ObservedToolResult {
    is_error: bool,
    text: String,
}
impl ObservedToolResult {
    fn text_content(&self) -> String {
        self.text.clone()
    }
}

#[derive(Default)]
struct ScriptedProvider(
    AtomicUsize,
    std::sync::Mutex<std::collections::VecDeque<(String, String, serde_json::Value)>>,
    std::sync::Mutex<std::collections::BTreeMap<String, ObservedToolResult>>,
    std::sync::Mutex<Vec<Vec<serde_json::Value>>>,
    std::sync::Mutex<Option<(String, String)>>,
);

fn observed_tool_results(
    messages: &[serde_json::Value],
) -> io::Result<Vec<(String, ObservedToolResult)>> {
    let mut results = Vec::new();
    for message in messages.iter().filter(|message| message["role"] == "user") {
        let Some(blocks) = message["content"].as_array() else {
            continue;
        };
        for block in blocks.iter().filter(|block| block["type"] == "tool_result") {
            let id = block["tool_use_id"]
                .as_str()
                .ok_or_else(|| io::Error::other("Native fixture tool result has no ID"))?;
            let text = block["content"]
                .as_str()
                .ok_or_else(|| io::Error::other("Native fixture tool result is not text"))?;
            let is_error = block["is_error"].as_bool().ok_or_else(|| {
                io::Error::other("Native fixture tool result has no error status")
            })?;
            results.push((
                id.into(),
                ObservedToolResult {
                    is_error,
                    text: text.into(),
                },
            ));
        }
    }
    Ok(results)
}

fn resumed_context_matches(messages: &[serde_json::Value], submitted: &str, notice: &str) -> bool {
    if notice.is_empty() {
        return false;
    }
    let Ok(results) = observed_tool_results(messages) else {
        return false;
    };
    let original_results = results
        .iter()
        .filter(|(id, _)| id == "submit-once")
        .collect::<Vec<_>>();
    let original_matches = original_results.len() == 1
        && !original_results[0].1.is_error
        && serde_json::from_str::<serde_json::Value>(&original_results[0].1.text_content())
            .is_ok_and(|value| {
                value["observation_json"] == submitted && value["recovery_only"] == true
            });
    // Count the complete native projection of the actual input, not a fact
    // substring that native may repeat within one SystemNotice. User text and
    // explicit text blocks are eligible; tool-result content is excluded.
    let notice_count = messages
        .iter()
        .filter(|message| message["role"] == "user")
        .map(|message| match &message["content"] {
            serde_json::Value::String(text) => text.matches(notice).count(),
            serde_json::Value::Array(blocks) => blocks
                .iter()
                .filter(|block| block["type"] == "text")
                .filter_map(|block| block["text"].as_str())
                .map(|text| text.matches(notice).count())
                .sum(),
            _ => 0,
        })
        .sum::<usize>();
    original_matches && notice_count == 1
}

#[test]
fn resumed_context_requires_one_complete_native_notice_projection() {
    use serde_json::json;
    let submitted = "original-observation";
    let body = r#"{"facts":{"persistence":"saved"}}"#;
    let mut header = meerkat_runtime::PromptInput::new(String::new(), None).header;
    header.source = meerkat_runtime::InputOrigin::External {
        source_name: "meerkat-connect-finalization".into(),
    };
    let input = meerkat_runtime::Input::ExternalEvent(meerkat_runtime::ExternalEventInput {
        header,
        event_type: "connect_finalization_observation".into(),
        payload: json!({"body":body}),
        blocks: None,
        handling_mode: meerkat_runtime::HandlingMode::default(),
        render_metadata: None,
        objective_id: None,
    });
    let Some(meerkat_core::types::ContentInput::Text(projection)) =
        meerkat_runtime::input::runtime_input_run_started_content(&input)
    else {
        panic!("the native no-media event must have a text projection")
    };
    assert!(!projection.is_empty());
    assert_ne!(
        projection, body,
        "the expected notice is the complete native projection"
    );
    let original = json!({"role":"user","content":[{"type":"tool_result",
        "tool_use_id":"submit-once","is_error":false,
        "content":json!({"observation_json":submitted,"recovery_only":true}).to_string()}]});
    for content in [
        json!(projection),
        json!([{"type":"text","text":projection}]),
    ] {
        let messages = vec![original.clone(), json!({"role":"user","content":content})];
        assert!(resumed_context_matches(&messages, submitted, &projection));
        assert!(!resumed_context_matches(&messages, submitted, ""));
    }
    // Missing, partial, altered and duplicated complete notices are refusals.
    for content in [
        json!(""),
        json!(body),
        json!(projection.replace("saved", "changed")),
        json!(format!("{projection}\n{projection}")),
        json!([{"type":"text","text":projection},{"type":"text","text":projection}]),
        json!([{"type":"tool_result","tool_use_id":"other-result","is_error":false,"content":projection}]),
    ] {
        let messages = vec![original.clone(), json!({"role":"user","content":content})];
        assert!(!resumed_context_matches(&messages, submitted, &projection));
    }
    let notice = json!({"role":"user","content":projection});
    assert!(!resumed_context_matches(
        &[original.clone(), notice.clone(), notice.clone()],
        submitted,
        &projection
    ));
    assert!(!resumed_context_matches(
        &[original.clone(), original, notice.clone()],
        submitted,
        &projection
    ));
    assert!(!resumed_context_matches(&[notice], submitted, &projection));
}

impl ScriptedProvider {
    fn reply(&self, body: serde_json::Value) -> io::Result<String> {
        let messages = body["messages"]
            .as_array()
            .ok_or_else(|| io::Error::other("Native fixture request has no messages"))?;
        let results = observed_tool_results(messages)?;
        self.0.fetch_add(1, Ordering::SeqCst);
        self.3.lock().unwrap().push(messages.clone());
        if let Some((submitted, notice)) = self.4.lock().unwrap().take() {
            // Gate this response on the first actual restored wire request,
            // never on results cached during the first input.
            if !resumed_context_matches(messages, &submitted, &notice) {
                // Failure-only observations of this same request. Emit no IDs,
                // result text, notice body or other conversation content.
                let original_results = results
                    .iter()
                    .filter(|(id, _)| id == "submit-once")
                    .collect::<Vec<_>>();
                let original_count = original_results.len();
                let original = (original_count == 1).then(|| &original_results[0].1);
                let original_is_error = original.map(|result| result.is_error);
                let original_json =
                    original.map(|result| serde_json::from_str::<serde_json::Value>(&result.text));
                let original_json_parsed = original_json.as_ref().map(|value| value.is_ok());
                let original_value = original_json.as_ref().and_then(|value| value.as_ref().ok());
                let observation_equal =
                    original_value.map(|value| value["observation_json"] == submitted);
                let recovery_only = original_value.map(|value| value["recovery_only"] == true);
                let notice_count = messages
                    .iter()
                    .filter(|message| message["role"] == "user")
                    .map(|message| match &message["content"] {
                        serde_json::Value::String(text) => text.matches(&notice).count(),
                        serde_json::Value::Array(blocks) => blocks
                            .iter()
                            .filter(|block| block["type"] == "text")
                            .filter_map(|block| block["text"].as_str())
                            .map(|text| text.matches(&notice).count())
                            .sum(),
                        _ => 0,
                    })
                    .sum::<usize>();
                let _ = writeln!(
                    io::stderr(),
                    "forge resumed-context mismatch: submit_result_count={original_count}; submit_is_error={original_is_error:?}; submit_json_parsed={original_json_parsed:?}; submit_observation_equal={observation_equal:?}; submit_recovery_only={recovery_only:?}; notice_occurrences={notice_count}"
                );
                return Err(io::Error::other(
                    "Native restored context is missing or duplicated",
                ));
            }
        }
        self.2.lock().unwrap().extend(results);
        if let Some((id, name, args)) = self.1.lock().unwrap().pop_front() {
            if !body["tools"].as_array().is_some_and(|tools| {
                tools
                    .iter()
                    .any(|tool| tool["name"].as_str() == Some(name.as_str()))
            }) {
                return Err(io::Error::other("Scripted native tool is not offered"));
            }
            return Ok(native_model_peer::tool_reply(&id, &name, &args));
        }
        Ok(native_model_peer::text_reply(ANSWER))
    }
}

// The existing native attachment fixture's line protocol, with the existing
// lazy-services setup_help schema. Native materialization owns this real stdio
// peer. No setup operation is requested by the scripted model.
const MCP_PEER: &str = r#"while IFS= read -r line; do
  id=$(printf '%s\n' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
  case "$line" in
    *'"method":"initialize"'*) printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":"2025-03-26","capabilities":{"tools":{}},"serverInfo":{"name":"stock-submission-fixture","version":"0"}}}\n' "$id" ;;
    *'"method":"tools/list"'*) printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[{"name":"setup_help","description":"Synthetic read only","inputSchema":{"type":"object","properties":{}},"annotations":{"readOnlyHint":true}}]}}\n' "$id" ;;
    *'"method":"ping"'*) printf '{"jsonrpc":"2.0","id":%s,"result":{}}\n' "$id" ;;
    *'"method":"tools/call"'*)
      printf '%s\n' "$line" >> "$1"
      printf '{"jsonrpc":"2.0","id":%s,"result":{"content":[{"type":"text","text":"unexpected fixture operation"}],"isError":true}}\n' "$id" ;;
  esac
done"#;

// Answer each actual protected prompt using only the ID the presenter rendered.
// No approval record or native grant is manufactured by the fixture.
struct ApproveDisplayed {
    bytes: Vec<u8>,
    answered: BTreeSet<String>,
    input: mpsc::Sender<io::Result<String>>,
}

impl Write for ApproveDisplayed {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        let text = std::str::from_utf8(&self.bytes).map_err(io::Error::other)?;
        for part in text.split("Type 'approve ").skip(1) {
            let Some((id, _)) = part.split_once('\'') else {
                continue;
            };
            if self.answered.insert(id.to_owned()) {
                self.input
                    .try_send(Ok(format!("approve {id}")))
                    .map_err(io::Error::other)?;
            }
        }
        Ok(())
    }
}

struct Opened {
    session: Option<ConnectSession>,
    store: Arc<dyn RuntimeStore>,
    stock: Arc<StockGovernance>,
    client: Arc<ScriptedProvider>,
    model_peer: Option<ModelPeer>,
    mcp_calls: std::path::PathBuf,
    setup: Arc<NativeSetup>,
    approvals: Arc<LocalApprovals>,
    connectors: crate::terminal_connector::TerminalConnectors,
    // A consuming close refusal is itself an owner and stays retained until
    // the fixture's separately observed exact teardown has finished.
    refused_close: Option<ConnectError>,
    continuation: StandaloneContinuation,
    approval_presenter: ApprovalPresenter,
    _host_presenter: crate::host_selection::HostSelectionPresenter,
    _directory: tempfile::TempDir,
}

impl Opened {
    async fn open() -> Result<Self, ConnectError> {
        Self::open_with_forge(None).await
    }

    async fn open_with_forge(
        forge: Option<(crate::forge::ForgeFrontend, crate::forge::ForgePresenter)>,
    ) -> Result<Self, ConnectError> {
        let client = Arc::new(ScriptedProvider::default());
        let observed = client.clone();
        let peer = ModelPeer::open(
            MODEL,
            Arc::new(move |body| Box::pin(std::future::ready(observed.reply(body)))),
        )
        .await
        .map_err(|e| ConnectError::native("Open native model fixture peer", e))?;
        use futures_util::FutureExt;
        let opened = std::panic::AssertUnwindSafe(Self::open_with_transport(forge, &peer, client))
            .catch_unwind()
            .await;
        match opened {
            Ok(Ok(mut opened)) => {
                opened.model_peer = Some(peer);
                Ok(opened)
            }
            Ok(Err(primary)) => combine(
                Err(primary),
                peer.finish()
                    .await
                    .map_err(|e| ConnectError::native("Join failed native model fixture peer", e)),
            ),
            Err(panic) => {
                let cleanup = peer.finish().await;
                assert!(
                    cleanup.is_ok(),
                    "Panicked native model fixture peer cleanup failed: {cleanup:?}"
                );
                std::panic::resume_unwind(panic)
            }
        }
    }

    async fn open_with_transport(
        forge: Option<(crate::forge::ForgeFrontend, crate::forge::ForgePresenter)>,
        peer: &ModelPeer,
        client: Arc<ScriptedProvider>,
    ) -> Result<Self, ConnectError> {
        // Forge recovery checks the setup artifact directory and its parent.
        // Declare this owned fixture root private instead of relying on umask.
        let directory = tempfile::Builder::new()
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir()
            .map_err(|e| ConnectError::native("fixture directory", e))?;
        let storage = crate::RealmOptions {
            context_root: directory.path().to_owned(),
            state_root: Some(directory.path().join("state")),
            user_config_root: Some(directory.path().join("home")),
        }
        .resolve(REALM)?;
        peer.configure(&storage)
            .await
            .map_err(|e| ConnectError::native("Configure isolated native model peer", e))?;
        let options = SessionOptions {
            model: native_model_peer::model_request(REALM, MODEL)
                .map_err(|e| ConnectError::native("Select exact native fixture binding", e))?,
            build: Default::default(),
        };
        let artifacts = directory.path().join("artifacts");
        std::fs::create_dir(&artifacts)
            .map_err(|e| ConnectError::native("fixture artifacts", e))?;
        let mcp_calls = directory.path().join("unexpected-mcp-calls");
        let descriptor = serde_json::to_vec(&serde_json::json!({
            "mcpServers": { "toolkit-setup": {
                "command": "/bin/sh",
                "args": ["-c", MCP_PEER, "stock-submission-control", mcp_calls.to_str().unwrap()]
            }}
        }))
        .map_err(|e| ConnectError::native("Encode actual fixture endpoint", e))?;
        let connection = crate::setup::PreparedSetupConnection::from_preparer_output(&descriptor)?;
        let prepared = crate::resources::PreparedResources {
            connection,
            artifact_directory: artifacts.clone(),
        };
        let native_session = Session::new();
        let id = native_session.id().clone();
        let (approvals, mut presenter) = LocalApprovals::create_paired_host(&artifacts)?;
        let approvals = Arc::new(approvals);
        approvals.bind_session(&id)?;
        let (send, receive) = mpsc::channel(1);
        let mut input = TerminalInput::scripted(receive);
        let mut output = ApproveDisplayed {
            bytes: Vec::new(),
            answered: BTreeSet::new(),
            input: send,
        };
        let interrupt = std::future::pending::<io::Result<()>>();
        tokio::pin!(interrupt);
        let mut startup = SessionStartup::prepare_standalone(
            storage.clone(),
            native_session,
            prepared,
            &approvals,
            &mut presenter,
            &mut input,
            &mut output,
            interrupt.as_mut(),
        )
        .await?
        .ok_or_else(|| invalid("The actual staged fixture was not opened"))?;
        let continuation = startup.standalone_continuation()?;
        let staged = startup
            .standalone
            .as_mut()
            .ok_or_else(|| invalid("Missing actual setup stage"))?;
        let stock = staged.stage.stock_governance().clone();
        let (send_store, receive_store) = oneshot::channel();
        staged.materialization_observer = Some(Box::new(move |bundle| {
            // This is the same store about to enter the production materializer.
            // The observer cannot replace the bundle or arm a construction fault.
            let _ = send_store.send(bundle.runtime_store());
        }));
        let configured = async {
            let mut connectors =
                crate::terminal_connector::TerminalConnectors::open(&mut startup).await?;
            let (paired_host, host_presenter) =
                crate::paired_host_tool::PairedHostFrontend::deferred();
            let setup = match forge {
                Some(forge) => NativeSetup::with_forge(
                    id,
                    storage.locator.clone(),
                    paired_host,
                    &connectors,
                    approvals.clone(),
                    forge,
                ),
                None => NativeSetup::new(
                    id,
                    storage.locator.clone(),
                    paired_host,
                    &connectors,
                    approvals.clone(),
                ),
            };
            let setup = match setup {
                Ok(setup) => setup,
                Err(primary) => return combine(Err(primary), connectors.retire()),
            };
            Ok::<_, ConnectError>((connectors, setup, host_presenter))
        }
        .await;
        let (mut connectors, setup, host_presenter) = match configured {
            Ok(value) => value,
            Err(primary) => {
                approvals.retire();
                return combine(Err(primary), startup.abort_standalone(&mut output).await);
            }
        };
        let result = Box::pin(startup.open_native_setup(
            options,
            setup.clone(),
            &approvals,
            &mut presenter,
            &mut input,
            &mut output,
            interrupt.as_mut(),
        ))
        .await;
        let session = match result {
            Ok(Some(session)) => session,
            other => {
                approvals.retire();
                let cleanup = connectors.retire();
                return combine(
                    other.and_then(|_| Err(invalid("The real native fixture did not open"))),
                    cleanup,
                );
            }
        };
        let observed = receive_store
            .await
            .map_err(|e| ConnectError::native("Observe actual finished bundle", e));
        let matches = session
            .stock
            .as_ref()
            .is_some_and(|owner| Arc::ptr_eq(owner, &stock));
        let store = match observed {
            Ok(store) if matches && output.answered.len() == 2 => store,
            observed => {
                approvals.retire();
                let primary = observed
                    .err()
                    .unwrap_or_else(|| invalid("Fixture did not retain its reviewed stage owner"));
                return combine(
                    Err(primary),
                    combine(session.close().await, connectors.retire()),
                );
            }
        };
        Ok(Self {
            session: Some(session),
            store,
            stock,
            client,
            model_peer: None,
            mcp_calls,
            setup,
            approvals,
            connectors,
            refused_close: None,
            continuation,
            approval_presenter: presenter,
            _host_presenter: host_presenter,
            _directory: directory,
        })
    }

    fn session(&self) -> Result<&ConnectSession, ConnectError> {
        self.session
            .as_ref()
            .ok_or_else(|| invalid("The fixture session was consumed"))
    }

    async fn finish(&mut self) -> Result<(), ConnectError> {
        self.approvals.retire();
        self.setup.retire();
        let closed = match self.session.take() {
            Some(session) => session.close().await,
            None => Ok(()), // The negative scenario retains and checks its own exact close.
        };
        let cleanup = combine(closed, self.connectors.retire());
        // Inspect only after the actual session shutdown. This does not stand
        // in for a process join or for live setup-server qualification.
        let calls = match std::fs::read(&self.mcp_calls) {
            Ok(bytes) => require(
                bytes.is_empty(),
                "The stock control invoked an unexpected MCP operation",
            ),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(ConnectError::native("Read fixture MCP calls", error)),
        };
        let peer = match self.model_peer.take() {
            Some(peer) => peer
                .finish()
                .await
                .map_err(|e| ConnectError::native("Join native model fixture peer", e)),
            None => Ok(()),
        };
        combine(cleanup, combine(calls, peer))
    }
}

fn require(value: bool, message: &str) -> Result<(), ConnectError> {
    if value { Ok(()) } else { Err(invalid(message)) }
}

fn retained_submission(error: &ConnectError) -> Result<&StockSubmissionError, ConnectError> {
    match error {
        ConnectError::Native {
            operation: "Governed submission ownership",
            source,
        } => source
            .downcast_ref::<StockSubmissionError>()
            .ok_or_else(|| invalid("The actual submission owner was not retained")),
        _ => Err(invalid(
            "The native submission returned a different error owner",
        )),
    }
}

async fn expect_refused(
    session: &ConnectSession,
    result: Result<(AcceptOutcome, CompletionHandle), ConnectError>,
) -> Result<ConnectError, ConnectError> {
    match result {
        Err(error) => Ok(error),
        Ok((admission, completion)) => {
            let cancelled = session.cancel(&admission).await.map(|_| ());
            let settled = completion
                .try_wait_with_terminal_outcome()
                .await
                .map(|_| ())
                .map_err(|e| ConnectError::native("Settle unexpected fixture admission", e));
            combine(cancelled, settled)?;
            Err(invalid("A refused submission was unexpectedly admitted"))
        }
    }
}

// Completion delivery and durable terminal publication are separate native
// observations. Wait only for these exact rows; never submit another input.
async fn settled_history(
    store: &dyn RuntimeStore,
    runtime: &LogicalRuntimeId,
    expected: &[meerkat_core::lifecycle::InputId],
) -> Result<Vec<meerkat_runtime::input_state::StoredInputState>, ConnectError> {
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let rows = store
                .load_input_states_strict(runtime)
                .await
                .map_err(|e| ConnectError::native("Read exact terminal input history", e))?;
            require(
                rows.len() == expected.len()
                    && expected
                        .iter()
                        .all(|id| rows.iter().any(|row| &row.state.input_id == id)),
                "Canonical history contains a missing or unexpected input occurrence",
            )?;
            if rows.iter().all(|row| row.seed.terminal_outcome.is_some()) {
                return Ok(rows);
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .map_err(|e| ConnectError::native("Wait for exact durable terminal observations", e))?
}

#[tokio::test]
async fn lost_commit_ack_keeps_actual_pending_original_through_unconfirmed_close() {
    fn diagnostic(stage: &'static str) {
        use std::io::Write;
        let _ = writeln!(std::io::stderr(), "lost-ack {stage}");
    }
    tokio::time::timeout(std::time::Duration::from_secs(60), async {
        diagnostic("open.entered");
        let mut fixture = Opened::open().await.expect("one actual staged native owner");
        diagnostic("open.returned");
        // Keep the actual published successor outside every fallible check so
        // a failed assertion cannot discard the only exact cleanup witness.
        let mut recovered_registration = None;
        let mut recovered_joined = false;
        let checked = async {
            let session = fixture.session()?;
            let fault = fixture.store.input_persist_ack_loss_fault_for_test()
                .ok_or_else(|| invalid("The actual stock store has no acknowledgement fault handle"))?;
            let runtime = LogicalRuntimeId::for_session(&session.session_id);
            fault.lose_next_commit_acknowledgement(runtime.clone());
            require(fault.is_armed(), "The exact runtime fault was not armed")?;
            diagnostic("first_submit.entered");
            let submitted = session.submit("First actual stock input".into()).await;
            diagnostic("first_submit.returned");
            diagnostic("first_refusal.entered");
            let error = expect_refused(session, submitted).await?;
            diagnostic("first_refusal.returned");
            let owner = retained_submission(&error)?;
            require(Arc::ptr_eq(&owner._custody, &session.stock_submission), "The failure lost its same submission owner")?;
            require(matches!(owner.source.as_ref(), ConnectError::Native { operation: "Submit governed setup message", .. }),
                "The injected native admission error was not retained")?;
            let input = {
                diagnostic("first_custody.entered");
                let guard = session.stock_submission.candidate.lock().await;
                diagnostic("first_custody.returned");
                let bound = guard.as_ref().ok_or_else(|| invalid("Committed unknown admission lost its bound input"))?;
                require(bound.pending.is_some() && bound._receipt.is_none(), "Unknown admission must retain its actual unpromoted candidate")?;
                require(bound.observation.get() == Some(&NativeInputAdmissionSettlement::Uncertain), "Commit acknowledgement loss was not classified uncertain")?;
                bound.input.clone()
            };
            require(!fault.is_armed() && fault.consumed_commit() == Some((runtime.clone(), vec![input.id().clone()])),
                "The fault did not consume this exact committed runtime/input")?;
            diagnostic("committed_rows.entered");
            let rows = fixture.store.load_input_states_strict(&runtime).await
                .map_err(|e| ConnectError::native("Read actual committed input", e))?;
            diagnostic("committed_rows.returned");
            require(rows.len() == 1 && rows[0].state.input_id == *input.id(), "The exact input was not durably committed")?;
            diagnostic("retry_submit.entered");
            let submitted = session.submit("Must not bind another occurrence".into()).await;
            diagnostic("retry_submit.returned");
            diagnostic("retry_refusal.entered");
            let retry = expect_refused(session, submitted).await?;
            diagnostic("retry_refusal.returned");
            require(Arc::ptr_eq(&retained_submission(&retry)?._custody, &session.stock_submission), "Retry refusal lost the original holder")?;
            {
                diagnostic("retry_custody.entered");
                let guard = session.stock_submission.candidate.lock().await;
                diagnostic("retry_custody.returned");
                let bound = guard.as_ref().ok_or_else(|| invalid("Retry removed the retained original"))?;
                require(bound.input.id() == input.id() && bound.pending.is_some(), "Retry replaced or disposed the pending original")?;
            }
            diagnostic("rechecked_rows.entered");
            let rows = fixture.store.load_input_states_strict(&runtime).await
                .map_err(|e| ConnectError::native("Recheck actual committed input", e))?;
            diagnostic("rechecked_rows.returned");
            require(rows.len() == 1 && rows[0].state.input_id == *input.id(), "A refused retry added a canonical input")?;

            let exact_machine = session.machine.clone();
            let exact_submission = session.stock_submission.clone();
            let exact_session = session.session_id.clone();
            let exact_epoch = session.registration.epoch_id().clone();
            // Commit acknowledgement loss requires actual durable reload;
            // ordinary unregister must first retain its repair-blocked result.
            diagnostic("original_unregister.entered");
            let unregistered = session.machine
                .unregister_session_registration_until_terminal_if_current(&session.registration).await;
            diagnostic("original_unregister.returned");
            let _reload_refusal = match unregistered {
                Err(error @ meerkat_runtime::RuntimeDriverError::RecoveryRepairBlocked { .. }) => error,
                Err(error) => return Err(ConnectError::native("Observe reload-required unregister refusal", error)),
                Ok(_) => return Err(invalid("Lost acknowledgement did not refuse ordinary unregister for reload")),
            };
            // Standalone stock binding creates a pending original, not a Mob
            // ops binding or OperationRetentionRequest. Preserve no invented
            // operation: the native store remains the cold recovery authority.
            diagnostic("exact_reload.entered");
            let settlement = session.machine.reload_required_registration_until_settled_if_current(
                &session.registration, &session.attachment, None,
            ).await;
            diagnostic("exact_reload.returned");
            let settlement = settlement.map_err(|e| ConnectError::native("Reload exact lost-ack registration", e))?;
            let successor = match settlement {
                meerkat_runtime::ReloadRequiredRegistrationSettlement::Discarded { successor } => successor,
                meerkat_runtime::ReloadRequiredRegistrationSettlement::NotCurrent =>
                    return Err(invalid("Lost-ack reload did not own the original registration")),
                meerkat_runtime::ReloadRequiredRegistrationSettlement::NotDegraded =>
                    return Err(invalid("Lost-ack reload did not observe durability degradation")),
            };
            recovered_registration = Some((exact_machine.clone(), successor));
            let (machine, successor) = recovered_registration.as_ref().unwrap();
            require(successor.session_id() == &exact_session && successor != &session.registration,
                "Reload did not return this session's distinct exact successor")?;
            diagnostic("successor_unregister.entered");
            let joined = machine.unregister_session_registration_until_terminal_if_current(successor).await;
            diagnostic("successor_unregister.returned");
            let joined = joined.map_err(|e| ConnectError::native("Join exact reloaded fixture registration", e))?;
            recovered_joined = joined;
            require(joined, "The fixture did not observe its exact terminal join")?;
            // Leave the original session's registration and attachment intact.
            // Its later consuming close sees false and must retain its own
            // unresolved submission rather than infer the successor's receipt.
            let session = fixture.session.take().ok_or_else(|| invalid("Missing same session owner"))?;
            diagnostic("stale_close.entered");
            let close = session.close().await;
            diagnostic("stale_close.returned");
            let error = close.err().ok_or_else(|| invalid("A later false close discarded the unresolved input"))?;
            fixture.refused_close = Some(error);
            let ConnectError::Native { source, .. } = fixture.refused_close.as_ref().unwrap() else {
                return Err(invalid("Unconfirmed close lost native owner custody"));
            };
            let retained = source.downcast_ref::<RetainedSetupCloseError>()
                .ok_or_else(|| invalid("Unconfirmed close did not retain the exact session"))?;
            require(matches!(retained.source.as_ref(), ConnectError::InvalidInput(message)
                if message == "This close did not confirm terminal teardown of the selected registration"),
                "Unconfirmed close must observe false unregister with successful service shutdown")?;
            require(Arc::ptr_eq(&exact_machine, &retained._session.machine)
                && Arc::ptr_eq(&exact_submission, &retained._session.stock_submission)
                && retained._session.registration.session_id() == &exact_session
                && retained._session.registration.epoch_id() == &exact_epoch,
                "Unconfirmed close replaced the actual machine, registration or submission owner")?;
            diagnostic("closed_retry_submit.entered");
            let submitted = retained._session.submit("No rebind after an unconfirmed close".into()).await;
            diagnostic("closed_retry_submit.returned");
            diagnostic("closed_retry_refusal.entered");
            let retry = expect_refused(&retained._session, submitted).await?;
            diagnostic("closed_retry_refusal.returned");
            require(Arc::ptr_eq(&retained_submission(&retry)?._custody, &retained._session.stock_submission), "Close refusal replaced the original input owner")?;
            diagnostic("retained_custody.entered");
            let guard = retained._session.stock_submission.candidate.lock().await;
            diagnostic("retained_custody.returned");
            let bound = guard.as_ref().ok_or_else(|| invalid("Close refusal disposed the pending original"))?;
            require(bound.input.id() == input.id() && bound.pending.is_some(), "The exact pending original did not survive false teardown")?;
            drop(guard);
            diagnostic("settled_rows.entered");
            let rows = fixture.store.load_input_states_strict(&runtime).await
                .map_err(|e| ConnectError::native("Read settled fixture history", e))?;
            diagnostic("settled_rows.returned");
            require(rows.len() == 1 && rows[0].state.input_id == *input.id(), "Close/retry changed the committed input occurrence")?;
            require(fault.consumed_commit() == Some((runtime, vec![input.id().clone()])), "Later cleanup changed the consumed fault identity")?;
            Ok::<_, ConnectError>(())
        }.await;
        diagnostic(if checked.is_ok() { "checked.ok" } else { "checked.failed" });
        let successor_cleanup = match recovered_registration.as_ref() {
            Some((machine, successor)) if !recovered_joined => {
                diagnostic("cleanup_successor_unregister.entered");
                let joined = machine.unregister_session_registration_until_terminal_if_current(successor).await;
                diagnostic("cleanup_successor_unregister.returned");
                joined.map_err(|e| ConnectError::native("Join retained reload successor during fixture cleanup", e))
                    .and_then(|joined| require(joined, "Cleanup did not confirm the retained successor's terminal join"))
            },
            _ => Ok(()),
        };
        diagnostic("fixture_cleanup.entered");
        let finished = fixture.finish().await;
        diagnostic("fixture_cleanup.returned");
        let cleanup = combine(successor_cleanup, finished);
        // Preserve the original custody/join failure as well as any cleanup
        // refusal; cleanup alone must not hide the first failed observation.
        combine(checked, cleanup).expect("lost acknowledgement must preserve actual stock custody and cleanup");
    }).await.expect("bounded real lost-acknowledgement control; timeout is not a cleanup receipt");
}

#[tokio::test]
async fn failed_actual_promotion_keeps_receipt_after_cancel_and_terminal_settlement() {
    tokio::time::timeout(std::time::Duration::from_secs(60), async {
        let mut fixture = Opened::open().await.expect("one actual staged native owner");
        let checked = async {
            let session = fixture.session()?;
            fixture.stock.fail_next_original_promotion_for_test();
            let error = expect_refused(session, session.submit("Actual admitted promotion failure".into()).await).await?;
            let retained = retained_submission(&error)?;
            require(Arc::ptr_eq(&retained._custody, &session.stock_submission), "Promotion failure lost same holder")?;
            let ConnectError::Native { operation: "Retain accepted setup original", source } = retained.source.as_ref() else {
                return Err(invalid("Promotion or its cancellation/settlement returned a different failure"));
            };
            require(matches!(source.downcast_ref::<StockCustodyError>(), Some(StockCustodyError::Unavailable)),
                "The actual one-shot native promotion refusal was not retained")?;
            let (input, receipt) = {
                let guard = session.stock_submission.candidate.lock().await;
                let bound = guard.as_ref().ok_or_else(|| invalid("Failed promotion lost its admitted input"))?;
                require(bound.pending.is_none(), "Consuming promotion failure restored its native candidate")?;
                require(bound.observation.get() == Some(&NativeInputAdmissionSettlement::Admitted), "Promotion was attempted without actual admitted custody")?;
                let Some((receipt, None)) = bound._receipt.as_ref() else { return Err(invalid("Failed promotion lost the exact receipt or did not join its completion")); };
                require(matches!(receipt, AcceptOutcome::Accepted { input_id, .. } if input_id == bound.input.id()), "The retained receipt names a different native input")?;
                (bound.input.clone(), receipt.clone())
            };
            let runtime = LogicalRuntimeId::for_session(&session.session_id);
            let rows = settled_history(fixture.store.as_ref(), &runtime, &[input.id().clone()]).await?;
            require(rows.len() == 1 && rows[0].state.input_id == *input.id() && rows[0].seed.terminal_outcome.is_some(),
                "Actual admitted input did not settle before returning promotion failure")?;
            // The production failure branch already cancelled and joined the
            // exact completion. Another explicit cancellation is not a rebind.
            session.cancel(&receipt).await?;
            let retry = expect_refused(session, session.submit("No rebind after failed promotion".into()).await).await?;
            require(Arc::ptr_eq(&retained_submission(&retry)?._custody, &session.stock_submission), "Settled cancellation enabled another holder")?;
            let guard = session.stock_submission.candidate.lock().await;
            let bound = guard.as_ref().ok_or_else(|| invalid("Cancellation cleared unresolved promotion"))?;
            require(bound.input.id() == input.id() && bound.pending.is_none() && bound._receipt.is_some(),
                "Cancellation replaced the exact unresolved input/receipt")?;
            drop(guard);
            let rows = fixture.store.load_input_states_strict(&runtime).await
                .map_err(|e| ConnectError::native("Recheck no promotion replay", e))?;
            require(rows.len() == 1 && rows[0].state.input_id == *input.id(), "Failed promotion retry added a native row")?;
            Ok::<_, ConnectError>(())
        }.await;
        let holder = fixture.session().unwrap().stock_submission.clone();
        let cleanup = fixture.finish().await;
        let released = holder.candidate.lock().await.is_none();
        assert!(cleanup.is_ok() && released, "only confirmed close releases unresolved promotion: {cleanup:?}");
        checked.expect("failed promotion must remain unresolved after actual cancellation settlement");
    }).await.expect("bounded real failed-promotion control; timeout is not a cleanup receipt");
}

#[tokio::test]
async fn successful_actual_stock_promotion_releases_slot_for_an_explicit_next_input() {
    tokio::time::timeout(std::time::Duration::from_secs(60), async {
        let mut fixture = Opened::open().await.expect("one actual staged native owner");
        let checked = async {
            let session = fixture.session()?;
            let runtime = LogicalRuntimeId::for_session(&session.session_id);
            let mut ids = Vec::new();
            let mut answers = Vec::new();
            for text in ["First explicit successful input", "Second explicit successful input"] {
                let (admission, completion) = session.submit(text.into()).await?;
                let empty = session.stock_submission.candidate.lock().await.is_none();
                // A positive control requires an ordinary successful native
                // turn, not merely acceptance followed by fixture cancellation.
                let observation = completion.try_wait_with_terminal_outcome().await
                    .map_err(|e| ConnectError::native("Join successful stock input", e))?;
                let (outcome, terminal) = observation.into_parts();
                let CompletionOutcome::Completed(result) = outcome else {
                    return Err(invalid("The actual promoted input did not complete successfully"));
                };
                require(terminal == Some(InputTerminalOutcome::Consumed) && result.text == ANSWER,
                    "The actual completed input did not return its scripted transport result")?;
                answers.push(result.text);
                require(empty, "Successful native promotion retained the submission slot")?;
                let AcceptOutcome::Accepted { input_id, .. } = admission else { return Err(invalid("Success did not return actual accepted custody")); };
                ids.push(input_id);
            }
            require(ids[0] != ids[1], "Explicit next input reused an earlier occurrence")?;
            let rows = settled_history(fixture.store.as_ref(), &runtime, &ids).await?;
            require(fixture.client.0.load(Ordering::SeqCst) == 2,
                "Both successful inputs must enter actual prepared authorization exactly once")?;
            for (id, answer) in ids.iter().zip(&answers) {
                let row = rows.iter().find(|row| &row.state.input_id == id)
                    .ok_or_else(|| invalid("Successful input has no exact canonical row"))?;
                require(row.seed.terminal_outcome == Some(InputTerminalOutcome::Consumed),
                    "Successful input has no durable consumed observation")?;
                // Consumed alone also describes abandoned inputs. Decode the
                // existing canonical terminal completion and correlate its
                // successful result to the live receipt for this exact ID.
                let value = serde_json::to_value(row)
                    .map_err(|e| ConnectError::native("Read canonical completion value", e))?;
                let outcome: CompletionOutcome = serde_json::from_value(value["terminal_completion"]["outcome"].clone())
                    .map_err(|e| ConnectError::native("Read canonical completion outcome", e))?;
                require(matches!(outcome, CompletionOutcome::Completed(result) if &result.text == answer),
                    "Durable terminal completion differs from the exact successful live input")?;
            }
            Ok::<_, ConnectError>(())
        }.await;
        let cleanup = fixture.finish().await;
        assert!(cleanup.is_ok(), "fixture cleanup failed: {cleanup:?}");
        checked.expect("actual successful promotion permits an explicit new input");
    }).await.expect("bounded real successful-promotion control; timeout is not a cleanup receipt");
}

// These controls enter the public Native Rust startup using the existing native
// fixture. The direct Toolkit dispatcher check below proves prompt custody only:
// it does not claim native tool admission, authenticated Forge or a live frontend.
fn inspect_native_forge_composition(
    setup: &NativeSetup,
    lazy: &crate::service_preparation::LazySetup,
    application: Option<&crate::forge::ForgeFrontend>,
) -> Result<(), ConnectError> {
    use futures_util::FutureExt;
    require(
        setup.take_composition().is_err(),
        "A native composition was handed out twice",
    )?;
    let before = lazy.dispatcher().tools();
    require(
        !before
            .iter()
            .any(|tool| tool.name.as_ref() == crate::forge::FORGE_PREPARE),
        "Forge was advertised before its protected presenter was transferred",
    )?;
    let mut presenter = lazy.take_forge_presenter()?;
    require(
        lazy.take_forge_presenter().is_err(),
        "The Forge receiver was handed out twice",
    )?;
    // The clone was retained before construction by the embedding application.
    // Matching it to the published receiver proves the original pair survived.
    let frontend = application.unwrap_or_else(|| lazy.forge());
    frontend.check_presenter(&presenter)?;
    require(
        frontend.claim_native_setup(&presenter).is_err(),
        "The native constructor did not claim its still-unused Forge binding",
    )?;
    let tools = lazy.dispatcher().tools();
    let prepare = tools
        .iter()
        .find(|tool| tool.name.as_ref() == crate::forge::FORGE_PREPARE)
        .ok_or_else(|| invalid("The native composition omitted Forge preparation"))?;
    let revision = &prepare.input_schema["properties"]["expected_revision"];
    let same_recipe = if application.is_some() {
        revision == &serde_json::json!({})
    } else {
        revision["type"] == "integer"
    };
    require(
        same_recipe,
        "The native composition replaced the selected application recipe",
    )?;
    // No paired host has been selected. Keep its initial selection/catalog
    // tools and service preparation live, without inventing authenticated tools.
    for name in [
        crate::host_selection::PAIRED_HOST_SELECT_HOST,
        crate::paired_host_tool::TOOLKIT_FRONTEND_CATALOG,
        crate::service_preparation::TOOLKIT_PREPARE_SERVICES,
    ] {
        require(
            before.iter().any(|tool| tool.name.as_ref() == name)
                && tools.iter().any(|tool| tool.name.as_ref() == name),
            "Selecting Forge removed an existing PairedHost or service operation",
        )?;
    }
    let paired_host_ceiling = lazy.paired_host().allowed_tools();
    for name in [
        crate::paired_host_tool::PAIRED_HOST_FRONTEND_INVENTORY,
        crate::paired_host_tool::PAIRED_HOST_FRONTEND_PREPARE,
    ] {
        require(
            paired_host_ceiling.contains(&name) && setup.bundle_operations().contains(name),
            "Selecting Forge removed a declared PairedHost operation",
        )?;
        require(
            !before
                .iter()
                .chain(tools.iter())
                .any(|tool| tool.name.as_ref() == name),
            "An unselected paired host exposed authenticated operations",
        )?;
    }
    require(
        lazy.installs().is_some() && lazy.owner().allows_acquisition(),
        "Selecting Forge removed the existing checked-install/service owner",
    )?;
    let dispatcher = frontend.dispatcher();
    let args = serde_json::value::RawValue::from_string("{}".into())
        .map_err(|error| ConnectError::native("Encode synthetic selection", error))?;
    let mut selection = Box::pin(dispatcher.dispatch(meerkat_core::types::ToolCallView {
        id: "native-forge-custody",
        name: crate::forge::FORGE_SELECT_HOST,
        args: &args,
    }));
    require(
        selection.as_mut().now_or_never().is_none(),
        "The selected frontend did not await its protected presenter",
    )?;
    let prompt = presenter
        .next()
        .now_or_never()
        .flatten()
        .ok_or_else(|| invalid("The original frontend request reached a different receiver"))?;
    // No origin or password is supplied. Dropping this exact offered prompt
    // retires its existing waiter before any HTTP or authenticated action.
    drop(prompt);
    require(
        matches!(selection.as_mut().now_or_never(), Some(Err(_))),
        "The retired pre-auth prompt did not settle its exact original waiter",
    )?;
    Ok(())
}

#[tokio::test]
async fn native_setup_keeps_application_forge_pair_through_public_startup() {
    let host = serde_json::from_value(serde_json::json!({
        "protocol":1, "host_id":format!("forge:{}", "a".repeat(64)),
        "application_id":"native-application", "source_digest":"b".repeat(64),
    }))
    .expect("declared synthetic application identity");
    let (frontend, presenter) =
        crate::forge::ForgeFrontend::for_application(host, ["PRIVATE_TOKEN".to_owned()].into())
            .expect("application-selected generic recipe");
    // Keep native ownership outside observer timeouts and assertions. Every
    // inspected result is propagated only after the exact fixture has closed.
    let mut fixture = Opened::open_with_forge(Some((frontend.clone(), presenter)))
        .await
        .expect("one actual native startup with the application-created pair");
    let checked = match fixture.setup.take_composition() {
        Ok((lazy, _services)) => {
            let result = inspect_native_forge_composition(&fixture.setup, &lazy, Some(&frontend));
            lazy.owner().retire();
            result
        }
        Err(error) => Err(error),
    };
    let cleanup = fixture.finish().await;
    assert!(cleanup.is_ok(), "fixture cleanup failed: {cleanup:?}");
    checked.expect("native composition must preserve the exact application frontend");
}

#[tokio::test]
async fn native_setup_default_keeps_reading_and_existing_service_paired_host_tools() {
    let mut fixture = Opened::open()
        .await
        .expect("one actual default native startup");
    let checked = match fixture.setup.take_composition() {
        Ok((lazy, _services)) => {
            let result = inspect_native_forge_composition(&fixture.setup, &lazy, None);
            lazy.owner().retire();
            result
        }
        Err(error) => Err(error),
    };
    let cleanup = fixture.finish().await;
    assert!(cleanup.is_ok(), "fixture cleanup failed: {cleanup:?}");
    checked.expect("default Reading, PairedHost and service composition must remain available");
}

#[path = "stock_submission_tests/forge_finalization.rs"]
mod forge_finalization;
