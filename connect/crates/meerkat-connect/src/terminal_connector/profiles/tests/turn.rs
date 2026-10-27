//! Real native input, tool feedback and permitted sibling through the exact
//! private main connector branch. Profile/config/approval owners are real.
//! The request adapter is a fixture: package installation selection and OAuth
//! are deliberately not claimed by this regression.
use super::*;
use crate::{ConnectSession, SessionOptions};
use async_trait::async_trait;
use futures_util::Stream;
use meerkat::{LlmClient, LlmError, LlmEvent, LlmRequest};
use meerkat_core::{AgentToolDispatcher, Message, Provider, SessionBuildOptions, ToolDef,
    ToolDispatchContext, ToolError, ToolMutationClass, ToolResult, ops::ToolDispatchOutcome,
    types::ToolCallView};
use tokio::sync::{mpsc, Notify};

struct RouteOutput { captured: Output, fail: bool }
impl Write for RouteOutput {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.fail { Err(io::Error::other("fixture terminal closed")) } else { self.captured.write(bytes) }
    }
    fn flush(&mut self) -> io::Result<()> { self.captured.flush() }
}

const PROFILE: &str = "toolkit_connect_service_account";
const SIBLING: &str = "profile_refusal_permitted_sibling";
struct Request { session: SessionId, response: oneshot::Sender<Value> }
struct Tools {
    requests: mpsc::Sender<Request>, sibling: AtomicUsize, abandon: Notify,
    secret: bool, secret_dispatcher: std::sync::OnceLock<Arc<dyn AgentToolDispatcher>>,
}
#[async_trait]
impl AgentToolDispatcher for Tools {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        let mut tools = [PROFILE, SIBLING].into_iter().map(|name| Arc::new(ToolDef::new(
            name, "Inert native profile-route regression", json!({"type":"object","additionalProperties":false}),
        ))).collect::<Vec<_>>();
        if self.secret { tools[0] = crate::terminal_connector::secrets::tool(); }
        tools.into()
    }
    fn tool_mutation_class(&self, name: &str) -> ToolMutationClass {
        if name == SIBLING { ToolMutationClass::ReadOnly } else { ToolMutationClass::Mutating }
    }
    async fn dispatch(&self, _call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
        Err(ToolError::execution_failed("native context required"))
    }
    async fn dispatch_with_context(&self, call: ToolCallView<'_>, context: &ToolDispatchContext) -> Result<ToolDispatchOutcome, ToolError> {
        if call.name == crate::terminal_connector::SECRET_TOOL {
            return self.secret_dispatcher.get().expect("selected native session was bound")
                .dispatch_with_context(call, context).await;
        }
        let session = context.origin_session_id().expect("actual native session").clone();
        let value = if call.name == PROFILE {
            let (response, receive) = oneshot::channel();
            self.requests.send(Request { session, response }).await.unwrap();
            tokio::select! { biased;
                _ = self.abandon.notified() => json!({"state":"unavailable","reason":"request_waiter_closed"}),
                result = receive => result.map_err(|_| ToolError::execution_failed("profile request ended"))?,
            }
        } else {
            assert_eq!(call.name, SIBLING);
            self.sibling.fetch_add(1, Ordering::SeqCst);
            json!({"sibling_completed":true})
        };
        Ok(ToolResult::new(call.id.into(), value.to_string(), false).into())
    }
}
struct Model {
    inner: meerkat_client::TestClient, issued: AtomicUsize,
    results: Mutex<std::collections::BTreeMap<String, meerkat_core::ToolResult>>,
    secret: bool,
}
#[async_trait]
impl LlmClient for Model {
    fn project_replay_messages(&self, messages: &[Message]) -> Result<Vec<Message>, LlmError> {
        self.inner.project_replay_messages(messages)
    }
    fn stream<'a>(&'a self, request: &'a LlmRequest) -> Pin<Box<dyn Stream<Item=Result<LlmEvent,LlmError>> + Send + 'a>> {
        for message in &request.messages {
            if let Message::ToolResults { results, .. } = message {
                for result in results { self.results.lock().unwrap().insert(result.tool_use_id.clone(), result.clone()); }
            }
        }
        let next = self.issued.fetch_add(1, Ordering::SeqCst);
        let (id, name) = match next {
            0 => ("profile", if self.secret { crate::terminal_connector::SECRET_TOOL } else { PROFILE }),
            1 => {
                let results = self.results.lock().unwrap();
                let result = results.get("profile").expect("feedback belongs to same native turn");
                // A stopped/lost presenter sends no ordinary refusal. Hold this
                // actual model continuation until the main route cancels it.
                if result.is_error { return Box::pin(futures_util::stream::pending()); }
                let value: Value = serde_json::from_str(&result.text_content()).unwrap();
                assert!(matches!(value["state"].as_str(), Some("denied" | "unavailable")));
                ("sibling", SIBLING)
            }
            _ => return self.inner.stream(request),
        };
        let args = if name == crate::terminal_connector::SECRET_TOOL {
            json!({"job_id":"3".repeat(32),"operations":["actual_read"],"agent":"rkat-agent","expires_in_ms":60000})
        } else { json!({}) };
        Box::pin(futures_util::stream::iter([
            Ok(LlmEvent::ToolCallComplete { id:id.into(), name:name.into(), args, meta:None }),
            Ok(LlmEvent::UsageUpdate { usage:meerkat_core::TurnUsage::host_declared(Provider::OpenAI,&request.model,meerkat_core::Usage::default()) }),
            Ok(LlmEvent::Done { outcome:meerkat::LlmDoneOutcome::success(meerkat::StopReason::ToolUse) }),
        ]))
    }
    fn provider(&self) -> Provider { Provider::OpenAI }
    async fn health_check(&self) -> Result<(), LlmError> { Ok(()) }
}

// Construct the large schedule future in a frame that returns before polling
// native session setup. The measured default-stack failure included 87,984
// bytes in the enclosing test poll frame despite the existing boxed open.
#[inline(never)]
fn profile_refusal_schedule(schedule: &'static str) -> Pin<Box<dyn std::future::Future<Output = ()>>> {
    Box::pin(async move {
        let directory = tempfile::tempdir().unwrap();
        let documents = schedule.starts_with("documents-");
        let storage = storage(directory.path(), if documents { "selected-owner" } else { "work" });
        let mut target = target(&storage); let source = native_source(&storage);
        let tokens = Arc::new(FileTokenStore::new(directory.path().join("credentials")));
        let persistence = ProviderAuthPersistence::new(tokens.clone(), Arc::new(FileLockCoordinator::new(directory.path().join("locks"))));
        if matches!(schedule, "incompatible" | "changed") {
            let mut client = definition(&target);
            if schedule == "incompatible" { client.client = "different-client".into(); }
            put(&storage, &persistence, client).await;
        }
        let snapshot = if schedule == "changed" { selected(&source,&target).await.unwrap() } else { None };
        let document_fixture = if documents { Some(crate::stdio::tests::selected_public_fixture(directory.path(),Some(json!({
            "authentication":{"kind":"none"},"operation_tools":{"declared/read":"actual_read"}}))).await) } else { None };
        let document_selection = document_fixture.as_ref().map(|fixture|fixture.raw_selection().ok().unwrap());
        if let Some(selected) = &document_selection { assert_eq!(selected.installed.realm, storage.locator); }
        let change_documents = || {
            let path=&document_selection.as_ref().unwrap().installed.receipt_path;
            if schedule.ends_with("missing") {std::fs::remove_file(path).unwrap()}
            else {std::fs::write(path,b"changed request document").unwrap()}
        };
        if schedule.starts_with("documents-before-") {change_documents();}
        let before = source.current_profiles().await.unwrap().into_iter().map(|p| p.binding()).collect::<Vec<_>>();
        let factory = meerkat::AgentFactory::new(directory.path().join("factory")).with_provider_auth_persistence(persistence.clone());
        let (send_requests, mut requests) = mpsc::channel(1);
        let tools = Arc::new(Tools { requests:send_requests, sibling:AtomicUsize::new(0), abandon:Notify::new(), secret:false, secret_dispatcher:Default::default() });
        let model = Arc::new(Model { inner:meerkat_client::TestClient::for_provider(Provider::OpenAI), issued:AtomicUsize::new(0), results:Mutex::new(Default::default()), secret:false });
        let options = SessionOptions {
            model:meerkat::CreateSessionModelResolutionRequest { model:Some("gpt-5.4".into()), ..Default::default() },
            build:SessionBuildOptions { external_tools:Some(tools.clone()), llm_client_override:Some(meerkat::encode_llm_client_override_for_service(model.clone())), ..Default::default() },
        };
        let session = Box::pin(ConnectSession::open_with_config(storage.clone(), meerkat_core::Config::default(), options)).await.unwrap();
        let (approvals, mut presenter) = LocalApprovals::create_paired_host(directory.path()).unwrap();
        approvals.bind_session(session.id()).unwrap();
        let (admission, completion) = session.submit("Try profile setup, then complete the permitted sibling if refused".into()).await.unwrap();
        let mut request = requests.recv().await.unwrap(); assert_eq!(&request.session,session.id());
        let (send_lines, receive_lines) = mpsc::channel(1);
        let mut input = TerminalInput::scripted(receive_lines); let seen=Output::default();
        let mut output=RouteOutput { captured:seen.clone(), fail:schedule=="output_failure" };
        let (stop, stopped) = oneshot::channel(); let mut stop=Some(stop);
        let interrupt = async { stopped.await.map_err(|_| io::Error::other("fixture stop source ended")) };
        tokio::pin!(interrupt);
        let observed_target=target.clone();
        let human = async {
            match schedule {
                "incompatible" | "output_failure" | "documents-before-changed" | "documents-before-missing" => {},
                "documents-after-changed" | "documents-after-missing" => {
                    shown(&seen,"Type 'approve ").await;
                    change_documents();
                    let text=seen.text();let id=text.split("Type 'approve ").last().unwrap().split('\'').next().unwrap();
                    send_lines.send(Ok(format!("approve {id}"))).await.unwrap();
                },
                "caller_closed" => {
                    shown(&seen,"Client authentication").await;
                    tools.abandon.notify_one();
                },
                "interrupt" => {
                    shown(&seen,"Client authentication").await;
                    stop.take().unwrap().send(()).unwrap();
                },
                "quit" => {
                    shown(&seen,"Client authentication").await;
                    send_lines.send(Ok("/quit".into())).await.unwrap();
                },
                "deny" => {
                    shown(&seen,"Client authentication").await; send_lines.send(Ok("public".into())).await.unwrap();
                    shown(&seen,"Advanced settings?").await; send_lines.send(Ok("".into())).await.unwrap();
                    shown(&seen,"Type 'approve ").await;
                    let text=seen.text(); let id=text.split("Type 'approve ").last().unwrap().split('\'').next().unwrap();
                    send_lines.send(Ok(format!("deny {id}"))).await.unwrap();
                },
                "changed" => {
                    shown(&seen,"Type 'approve ").await;
                    let mut client=definition(&observed_target); client.resource_indicator=false;
                    put(&storage,&persistence,client).await;
                    let text=seen.text(); let id=text.split("Type 'approve ").last().unwrap().split('\'').next().unwrap();
                    send_lines.send(Ok(format!("approve {id}"))).await.unwrap();
                },
                _ => unreachable!(),
            }
        };
        let presenting = async {
            if let Some(selected) = &document_selection {
                // The adapter is controlled, while the selected document read,
                // approval owner, main branch, admitted native input and sibling
                // consumption are real. Actual presenter placements are covered
                // separately by the public/account review controls.
                if schedule.starts_with("documents-after-") {
                    let request_body=review_request(session.id(),"Review selected documents",json!({"phase":"document-regression"})).unwrap();
                    match super::super::review(&approvals,&mut presenter,&mut input,&mut output,request_body,
                        &mut request.response,interrupt.as_mut()).await? {
                        ReviewOutcome::Decided(record) => assert_eq!(record.status,ApprovalStatus::Approved),
                        _ => panic!("document scenario must approve its exact review"),
                    }
                }
                let reason=crate::terminal_connector::request_documents(
                    crate::terminal_connector::plain::recheck_retained(&selected.installed,&selected.configuration,selected.selected_inputs())
                ).await.expect_err("the selected document must be refused");
                assert_eq!(reason,"selected_documents_unconfirmed");
                return crate::terminal_connector::selection_refused(reason,request.response,&mut output);
            }
            let result = if schedule == "changed" {
                let review = review_request(session.id(),"Review installed account access",json!({"phase":"account_access","client_profile":display(snapshot.as_ref())})).unwrap();
                let interrupt = std::future::pending::<std::io::Result<()>>();
                tokio::pin!(interrupt);
                match super::super::review(&approvals,&mut presenter,&mut input,&mut output,review,&mut request.response,interrupt.as_mut()).await {
                    Ok(ReviewOutcome::Decided(record)) => {
                        assert_eq!(record.status,ApprovalStatus::Approved);
                        recheck(&source,&target,snapshot.as_ref()).await.map(|()| Preparation::Current(snapshot.clone()))
                    },
                    Ok(ReviewOutcome::CallerGone) => Ok(Preparation::CallerGone),
                    Ok(ReviewOutcome::Stopped) => Ok(Preparation::Stopped),
                    Err(error) => Err(error.into()),
                }
            } else {
                prepare(&storage,&factory,&source,&mut target,session.id(),&approvals,&mut presenter,&mut input,&mut output,&mut request.response,||Ok(()),interrupt.as_mut()).await
            };
            match presentation(result,request.response,&mut output) {
                Ok(Presentation::Finished(keep)) => Ok(keep),
                Ok(Presentation::Current(..)) => panic!("this control must refuse before OAuth"),
                Err(error) => Err(error),
            }
        };
        let (presented,())=tokio::join!(presenting,human);
        let observed=Arc::new(Mutex::new(None)); let result=observed.clone();
        let waiting=async move { let outcome=completion.wait().await.unwrap(); *result.lock().unwrap()=Some(outcome); };
        tokio::pin!(waiting);
        // This is the exact private function called by main.rs, including
        // its sole cancellation/join branch, not a duplicated test policy.
        let routed=crate::main_connector::continue_after_presentation(presented,&session,&admission,&approvals,waiting.as_mut()).await;
        let kept = match routed {
            Ok(kept) => { assert_ne!(schedule,"output_failure"); kept },
            Err(crate::ConnectError::Native { operation, .. }) if schedule=="output_failure" => {
                assert_eq!(operation,"Display client setup"); false
            },
            other => panic!("unexpected main route outcome: {other:?}"),
        };
        if kept { waiting.await; }
        let session_id = session.id().clone();
        session.close().await.unwrap();
        let stopping=matches!(schedule,"quit"|"interrupt"|"output_failure");
        assert_eq!(kept,!stopping);
        let outcome=observed.lock().unwrap().take().unwrap();
        if stopping {
            assert!(matches!(outcome,meerkat_runtime::completion::CompletionOutcome::Cancelled));
            assert_eq!(tools.sibling.load(Ordering::SeqCst),0);
        } else {
            assert!(matches!(outcome,meerkat_runtime::completion::CompletionOutcome::Completed(_)));
            assert_eq!(tools.sibling.load(Ordering::SeqCst),1);
            let results=model.results.lock().unwrap();
            assert!(!results["profile"].is_error); assert!(!results["sibling"].is_error);
            let value:Value=serde_json::from_str(&results["profile"].text_content()).unwrap();
            assert_eq!(value["state"],if schedule=="deny" {"denied"} else {"unavailable"});
            if documents {assert_eq!(value["reason"],"selected_documents_unconfirmed");}
            approvals.check_session(&session_id).unwrap();
            assert!(!approvals.has_uncertain_dispatch());
        }
        assert!(tokens.list().await.unwrap().is_empty(),"refusal never saves a secret");
        let after=source.current_profiles().await.unwrap();
        if schedule=="changed" { assert!(!after[0].resource_indicator); }
        else { assert_eq!(after.into_iter().map(|p|p.binding()).collect::<Vec<_>>(),before); }
        assert!(!seen.text().contains("Open this private sign-in URL"));
    })
}

#[tokio::test]
async fn profile_refusals_reach_same_native_turn_and_sibling_while_explicit_stop_cancels() {
    for schedule in ["deny", "incompatible", "changed", "caller_closed", "quit", "interrupt", "output_failure",
        "documents-before-changed","documents-before-missing","documents-after-changed","documents-after-missing"] {
        tokio::time::timeout(Duration::from_secs(30), profile_refusal_schedule(schedule))
            .await.expect("same admitted native turn must settle under the original fixture bound");
    }
}

// Use the same model and exact main branch as the profile control, but route
// the request through the production secret dispatcher and presenter. The
// selected-package fixture supplies inert checked documents, not an install.
#[inline(never)]
fn initial_secret_refusal_schedule(inherited: bool) -> Pin<Box<dyn std::future::Future<Output = ()>>> {
    Box::pin(async move {
        use crate::{SessionStartup, terminal_connector::TerminalConnectors};
        use meerkat_core::{ConfigDelta, ConfigRuntime, ConfigStore, FileConfigStore, McpRealmPersistTarget};
        use std::panic::{AssertUnwindSafe, resume_unwind};

        let directory = tempfile::tempdir().unwrap();
        let storage = storage(directory.path(), "selected-owner");
        let tokens = Arc::new(FileTokenStore::new(directory.path().join("credentials")));
        let persistence = ProviderAuthPersistence::new(tokens.clone(),
            Arc::new(FileLockCoordinator::new(directory.path().join("locks"))));
        let mut startup = SessionStartup { storage:storage.clone(), config:meerkat_core::Config::default(),
            factory:meerkat::AgentFactory::new(directory.path().join("factory"))
                .builtins(false).shell(false).with_provider_auth_persistence(persistence),
            persistence:None, connector_auth:None, standalone:None };
        let mut connectors = Box::pin(TerminalConnectors::open(&mut startup)).await.unwrap();
        let (send_requests, _unused_profile_requests) = mpsc::channel(1);
        let tools = Arc::new(Tools { requests:send_requests, sibling:AtomicUsize::new(0),
            abandon:Notify::new(), secret:true, secret_dispatcher:Default::default() });
        let model = Arc::new(Model { inner:meerkat_client::TestClient::for_provider(Provider::OpenAI),
            issued:AtomicUsize::new(0), results:Mutex::new(Default::default()), secret:true });
        let options = SessionOptions {
            model:meerkat::CreateSessionModelResolutionRequest { model:Some("gpt-5.4".into()), ..Default::default() },
            build:SessionBuildOptions { external_tools:Some(tools.clone()),
                llm_client_override:Some(meerkat::encode_llm_client_override_for_service(model.clone())), ..Default::default() },
        };
        // Consume the original startup bundle used by the terminal owner. Do
        // not open another runtime or infer a session from fixture coordinates.
        let session = Box::pin(ConnectSession::fixture_open_governed(startup, options)).await.unwrap();
        let mut admitted = None;
        let settled = Arc::new(AtomicBool::new(false));
        let observed = Arc::new(Mutex::new(None));
        let checked = tokio::time::timeout(Duration::from_secs(30), AssertUnwindSafe(async {
            let fixture = crate::stdio::tests::selected_public_fixture_for_session(directory.path(), json!({
                "authentication":{"kind":"binding","owner":"process","requirement":"key","credential_audience":"service"},
                "operation_tools":{"declared/read":"actual_read"},"environment_secret_slots":["FIXTURE_KEY"]
            }), session.id().clone()).await;
            let selected = fixture.raw_selection().ok().unwrap();
            assert_eq!(&selected.installed.session, session.id());
            assert_eq!(selected.installed.realm, storage.locator);
            selected.check_current().unwrap();
            let config = selected.secret_configuration(&["actual_read".to_owned()].into()).unwrap();
            crate::terminal_connector::plain::recheck_retained(&selected.installed,
                &selected.configuration, selected.selected_inputs()).await.unwrap();
            assert!(tools.secret_dispatcher.set(connectors.dispatcher(fixture.owner.clone())).is_ok());

            let own_path = crate::realm::config_source(&storage).unwrap().config_doc_path(&storage.locator.realm);
            let parent_path = storage.layout.user_rkat_root().unwrap().join("config.toml");
            let occupied_path = if inherited { parent_path.clone() } else { own_path.clone() };
            let store = Arc::new(FileConfigStore::new(occupied_path.clone(), meerkat_models::canonical()));
            if inherited {
                // Declare the native implicit parent without replacing its other fields.
                store.patch(ConfigDelta(json!({"realm":{"global":{}}}))).await.unwrap();
            }
            let writer = ConfigRuntime::new(store, occupied_path.with_file_name("config_state.json"));
            McpRealmPersistTarget::new(&writer).add(config.clone(), Some(0)).await.unwrap();
            drop(writer);
            let own_before = crate::realm::read_own_mcp_config(&storage).await.unwrap();
            assert_eq!(own_before.0.iter().any(|server| server.name == config.name), !inherited);
            assert!(crate::realm::read_config(&storage).await.unwrap().tools.mcp_servers.contains(&config),
                "the real effective view must contain the occupied target");
            let realm = meerkat_store::realm_paths_in(&storage.locator.state_root, storage.locator.realm.as_str());
            let paths = [own_path.clone(), own_path.with_file_name("config_state.json"),
                parent_path.clone(), parent_path.with_file_name("config_state.json"),
                meerkat_store::grant_owner_store_path(&realm.root)];
            let snapshot = || paths.iter().map(|path| match std::fs::read(path) {
                Ok(bytes) => Some(bytes),
                Err(error) if error.kind() == io::ErrorKind::NotFound => None,
                Err(error) => panic!("read fixture state: {error}"),
            }).collect::<Vec<_>>();
            let before = snapshot();
            assert!(tokens.list().await.unwrap().is_empty());

            let (approvals, mut presenter) = LocalApprovals::create_paired_host(directory.path()).unwrap();
            approvals.bind_session(session.id()).unwrap();
            let (admission, completion) = session.submit("Try initial secret setup; if refused, complete the permitted sibling".into()).await.unwrap();
            let result = observed.clone();
            let finished = settled.clone();
            admitted = Some((admission, Box::pin(async move {
                let outcome = completion.wait().await;
                finished.store(true, Ordering::SeqCst);
                *result.lock().unwrap() = Some(outcome);
            })));
            let (admission, waiting) = admitted.as_mut().unwrap();
            let request = connectors.next().await.expect("production secret request reached its presenter");
            assert!(matches!(&request, crate::terminal_connector::ConnectorRequest::Secret(_)));
            let pair = openpty(None, None).unwrap();
            let mut input = TerminalInput::from_owned_fd(pair.slave);
            let mut output = Output::default();
            let presented = connectors.present(request, &approvals, &mut presenter, &mut input, &mut output,
                std::future::pending::<io::Result<()>>()).await;
            let kept = crate::main_connector::continue_after_presentation(presented, &session, admission,
                &approvals, waiting.as_mut()).await.expect("occupied secret target must remain operation-local");
            assert!(kept);
            waiting.as_mut().await;
            assert!(matches!(observed.lock().unwrap().take().unwrap().unwrap(),
                meerkat_runtime::completion::CompletionOutcome::Completed(_)));
            assert_eq!(tools.sibling.load(Ordering::SeqCst), 1);
            {
                let results = model.results.lock().unwrap();
                assert!(!results["profile"].is_error);
                assert!(!results["sibling"].is_error);
                let feedback: Value = serde_json::from_str(&results["profile"].text_content()).unwrap();
                assert!(matches!(feedback["state"].as_str(), Some("denied" | "unavailable")));
                assert_eq!(feedback["grant_issued"], false);
            }
            approvals.check_session(session.id()).unwrap();
            assert!(!approvals.has_uncertain_dispatch());
            assert!(!connectors.has_reviewed());
            assert!(!output.text().contains("Type 'approve "));
            assert!(!output.text().contains("Credential for "));
            input.check_cleanup().unwrap();
            assert_eq!(crate::realm::read_own_mcp_config(&storage).await.unwrap(), own_before);
            assert_eq!(snapshot(), before, "no own/inherited config or grant mutation");
            assert!(tokens.list().await.unwrap().is_empty());
        }).catch_unwind()).await;
        // The observer only borrows admitted work. Cancel and join an unsettled
        // input before close; the normal RED already joined it in the main route.
        let retired = connectors.retire();
        let cancelled = if !settled.load(Ordering::SeqCst) {
            if let Some((admission, waiting)) = admitted.as_mut() {
                let cancelled = session.cancel(admission).await;
                waiting.as_mut().await;
                Some(cancelled)
            } else { None }
        } else { None };
        let closed = tokio::time::timeout(Duration::from_secs(10), session.close()).await;
        // Preserve the original observer failure after cleanup was attempted.
        // A timeout is never labeled a successful native close.
        match checked {
            Ok(Ok(())) => {},
            Ok(Err(panic)) => resume_unwind(panic),
            Err(error) => panic!("secret refusal main route did not settle: {error}"),
        }
        if let Some(cancelled) = cancelled { cancelled.unwrap(); }
        retired.unwrap();
        closed.expect("secret refusal fixture close must settle").unwrap();
    })
}

#[tokio::test]
async fn occupied_own_initial_secret_target_keeps_same_native_turn_and_sibling_without_writes() {
    initial_secret_refusal_schedule(false).await;
}

#[tokio::test]
async fn occupied_inherited_initial_secret_target_keeps_same_native_turn_and_sibling_without_writes() {
    initial_secret_refusal_schedule(true).await;
}
