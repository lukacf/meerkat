//! Actual staged-owner refusal and fresh authenticated host probe controls.
//! These do not qualify an installed finalizer or protected commissioning flow.
use super::*;

#[tokio::test]
async fn private_caller_cannot_borrow_the_terminal_operator_input_owner() {
    let mut fixture = Opened::open()
        .await
        .expect("actual reviewed staged native setup");
    let body = std::panic::AssertUnwindSafe(async {
        let session = fixture.session()?;
        let runtime = LogicalRuntimeId::for_session(session.id());
        let before = fixture
            .store
            .load_input_states_strict(&runtime)
            .await
            .map_err(|e| ConnectError::native("Read native control inputs", e))?;
        let caller = crate::service_probe::tests::caller();
        let refused = expect_refused(
            session,
            session
                .submit_input_for_caller(
                    meerkat_runtime::Input::Prompt(meerkat_runtime::PromptInput::new(
                        "private caller",
                        None,
                    )),
                    Some(&caller),
                )
                .await,
        )
        .await?;
        let after = fixture
            .store
            .load_input_states_strict(&runtime)
            .await
            .map_err(|e| ConnectError::native("Read refused native control inputs", e))?;
        require(
            serde_json::to_value(&before).unwrap() == serde_json::to_value(&after).unwrap(),
            "The foreign private caller changed native input state",
        )?;
        drop(refused);
        Ok::<_, ConnectError>(())
    })
    .catch_unwind()
    .await;
    let cleanup = fixture.finish().await;
    assert!(
        cleanup.is_ok(),
        "Exact native control cleanup failed: {cleanup:?}"
    );
    match body {
        Ok(result) => result.unwrap(),
        Err(panic) => std::panic::resume_unwind(panic),
    }
}

// The exact same prepared native controller and MCP grant producers used by
// the standalone controls. This peer executes a real native MCP call and emits
// only an inert result; no installed resource readiness is asserted.
const READ_PEER: &str = r#"while IFS= read -r line; do
  id=$(printf '%s\n' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
  case "$line" in
    *'"method":"initialize"'*) printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":"2025-03-26","capabilities":{"tools":{}},"serverInfo":{"name":"private-probe-control","version":"0"}}}\n' "$id" ;;
    *'"method":"tools/list"'*) printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[{"name":"setup_help","description":"Read the inert selected fixture resource","inputSchema":{"type":"object","properties":{}},"annotations":{"readOnlyHint":true}}]}}\n' "$id" ;;
    *'"method":"ping"'*) printf '{"jsonrpc":"2.0","id":%s,"result":{}}\n' "$id" ;;
    *'"method":"tools/call"'*)
      printf '%s\n' "$line" >> "$1"
      printf '{"jsonrpc":"2.0","id":%s,"result":{"content":[{"type":"text","text":"exact private fixture resource"}],"isError":false}}\n' "$id" ;;
  esac
done"#;

#[tokio::test]
async fn fresh_authenticated_host_probe_commits_actual_native_call_and_refuses_after_retirement() {
    use crate::service_probe::{ProbeAccess, RequiredServiceProbe, ServiceProbeSelection};
    use crate::session::service_probe::{ProbeGate, probe_turn};
    use meerkat::governed_realm::{
        open_stock_realm_persistence_for_host,
        stock::{StockPrincipals, issuance::stock_agent_principal},
    };
    let client = Arc::new(ScriptedProvider::default());
    let responses = client.clone();
    let peer = ModelPeer::open(
        MODEL,
        Arc::new(move |body| Box::pin(std::future::ready(responses.reply(body)))),
    )
    .await
    .unwrap();
    let mut original = None;
    let mut active = None;
    let mut root = None;
    let mut selected_host = None;
    let mut phase = "open_original";
    let body = std::panic::AssertUnwindSafe(async {
        original =
            Some(Opened::open_with_selected_mcp(None, &peer, client.clone(), READ_PEER).await?);
        let old = original.as_mut().unwrap();
        let storage = old.continuation.storage.clone();
        let id = old.session()?.id().clone();
        let server = old.continuation.server.clone();
        let calls = old.mcp_calls.clone();
        let native = old.session.take().unwrap();
        phase = "close_original";
        native.close().await?;
        old.approvals.retire();
        old.setup.retire();
        old.connectors.retire()?;
        // Move the filesystem root only; every original router/auth/presenter
        // alias is dropped before opening the actual fresh native host owner.
        let Opened {
            session,
            store,
            stock,
            client: old_client,
            model_peer,
            mcp_calls: old_calls,
            setup,
            approvals,
            connectors,
            refused_close,
            continuation,
            approval_presenter,
            _host_presenter,
            _directory,
        } = original.take().unwrap();
        drop((
            session,
            store,
            stock,
            old_client,
            model_peer,
            old_calls,
            setup,
            approvals,
            connectors,
            refused_close,
            continuation,
            approval_presenter,
            _host_presenter,
        ));
        root = Some(_directory);
        phase = "open_private_owner";
        let mut binding = crate::service_probe::tests::binding(root.as_ref().unwrap().path());
        binding.realm = storage.locator.clone();
        binding.context_root = root.as_ref().unwrap().path().to_path_buf();
        binding.user_config_root = root.as_ref().unwrap().path().join("home");
        let profile = RequiredServiceProbe::outbound_mcp(
            "native-read".into(),
            "fixture".into(),
            "read".into(),
            DEFAULT_STOCK_AGENT.into(),
            "setup_help".into(),
            serde_json::json!({}),
            serde_json::json!({"fixture":"selected"}),
            None,
        )?;
        let host = Arc::new(ServiceProbeSelection::new(binding, vec![profile])?);
        selected_host = Some(host.clone());
        let profile = host.profiles[0].clone();
        let principals = StockPrincipals::for_local_operator(&storage.locator.realm).unwrap();
        let access = Arc::new(ProbeAccess {
            lifetime: host.lifetime.clone(),
            expires_at_ms: host.binding.expires_at_ms,
            caller: host.binding.caller.clone(),
            executor: stock_agent_principal(&storage.locator.realm, DEFAULT_STOCK_AGENT).unwrap(),
            owner: principals.operator,
            session: id.clone(),
        });
        let (_, persistence) = open_stock_realm_persistence_for_host(
            &meerkat::DiskStorageProvider,
            &storage.locator.state_root,
            storage.locator.realm.as_str(),
            None,
            Some(meerkat_store::RealmOrigin::Explicit),
            Some(storage.layout.clone()),
            DEFAULT_STOCK_AGENT,
            host.binding.ingress.clone(),
            access.clone(),
        )
        .await
        .map_err(|error| ConnectError::native("Open actual private host control", error))?;
        let stock = persistence
            .stock_governance()
            .cloned()
            .ok_or_else(|| invalid("Missing actual host governance"))?;
        host.lifetime.join(stock.authorization_publication())?;
        let committed = persistence
            .session_store()
            .load(&id)
            .await
            .map_err(|error| ConnectError::native("Load actual closed control session", error))?
            .ok_or_else(|| invalid("Actual committed control session absent"))?;
        let startup = SessionStartup::open(storage.clone()).await?;
        let mut config = startup.config.clone();
        config.tools.mcp_servers = vec![server.clone()];
        let model = meerkat::resolve_create_session_model(
            &config,
            native_model_peer::model_request(REALM, MODEL).unwrap(),
        )
        .map_err(|error| ConnectError::native("Select control model", error))?;
        let gate = ProbeGate::new(access, profile, "setup_help".into());
        let mut build = meerkat_core::SessionBuildOptions {
            mcp_servers: vec![server.clone()],
            realm_id: Some(storage.locator.realm.clone()),
            provider: Some(model.provider),
            auth_binding: model.auth_binding,
            ..Default::default()
        };
        build.resume_existing_session(committed.clone());
        build.tool_dispatch_admission = Some(gate.clone());
        build.tool_access_policy = Some(meerkat_core::ToolAccessPolicy::AllowList(
            ["setup_help".to_owned()].into_iter().collect(),
        ));
        let mut builder = FactoryAgentBuilder::new(startup.factory.clone(), config);
        builder.wait_for_mcp = true;
        let request = CreateSessionRequest {
            model: model.model,
            prompt: String::new().into(),
            injected_context: Vec::new(),
            system_prompt: meerkat::SystemPromptOverride::Inherit,
            max_tokens: None,
            event_tx: None,
            initial_turn: meerkat_core::service::InitialTurnPolicy::Defer,
            deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::Discard,
            build: Some(build),
            labels: None,
        };
        phase = "materialize_private_session";
        active = Some(
            ConnectSession::materialize_governed(
                &storage,
                builder,
                persistence,
                committed,
                request,
                stock,
                None,
                None,
                None,
                &[server.name],
            )
            .await?,
        );
        client.1.lock().unwrap().push_back((
            "private-probe-read".into(),
            "setup_help".into(),
            serde_json::json!({}),
        ));
        phase = "first_probe";
        let observation = probe_turn(active.as_ref().unwrap(), &gate, &host)
            .await
            .map_err(|(_, error)| error)?;
        phase = "verify_committed_call";
        require(
            !observation.result.is_error
                && observation.result.text_content() == "exact private fixture resource",
            "The native call did not return the selected fixture resource",
        )?;
        require(
            observation.dispatch_outcome == meerkat_core::LiveBridgeEffectOutcome::Committed,
            "The actual native effect was not committed",
        )?;
        require(
            std::fs::read_to_string(&calls).unwrap().lines().count() == 1,
            "Expected one real MCP dispatch",
        )?;
        phase = "retired_probe_refusal";
        host.control().retire();
        let before = std::fs::read(&calls).unwrap();
        require(
            probe_turn(active.as_ref().unwrap(), &gate, &host)
                .await
                .is_err(),
            "Retired host admitted another input",
        )?;
        require(
            std::fs::read(&calls).unwrap() == before,
            "Retired host dispatched a second physical operation",
        )?;
        let machine = active.as_ref().unwrap().machine.clone();
        phase = "close_private_session";
        active.take().unwrap().close().await?;
        phase = "durable_terminal_receipt";
        use meerkat_runtime::terminal_status::{
            InputTerminalReceiptObservation, InteractionSelector, TerminalWitnessSource,
        };
        let terminal = machine
            .input_terminal_receipt_observation(
                &id,
                InteractionSelector::InputId(observation.input.clone()),
            )
            .await
            .map_err(|error| ConnectError::native("Read actual native probe terminal", error))?
            .ok_or_else(|| invalid("No native terminal"))?;
        require(
            terminal.source == TerminalWitnessSource::DurableStore,
            "Native result was not durably terminal",
        )?;
        let InputTerminalReceiptObservation::Finalized(receipt) = terminal.report else {
            return Err(invalid("Native receipt not finalized"));
        };
        require(
            receipt.input_id() == &observation.input
                && receipt.owner_input_id() == &observation.input
                && receipt.run_id() == Some(&observation.run)
                && receipt.terminal() == &InputTerminalOutcome::Consumed,
            "Native receipt did not name the actual probe input/run",
        )?;
        phase = "complete";
        Ok::<_, ConnectError>(())
    })
    .catch_unwind()
    .await;
    if let Some(host) = selected_host {
        host.control().retire();
    }
    // Attempt every actual close even if a prior close panics. These results
    // retain any consuming native error owner through all later cleanup and
    // private evidence writing; dropping an error is never a successful join.
    let native_cleanup = std::panic::AssertUnwindSafe(async {
        match active.take() {
            Some(session) => session.close().await,
            None => Ok(()),
        }
    })
    .catch_unwind()
    .await;
    let original_cleanup = std::panic::AssertUnwindSafe(async {
        match original.as_mut() {
            Some(fixture) => fixture.finish().await,
            None => Ok(()),
        }
    })
    .catch_unwind()
    .await;
    let peer_cleanup = std::panic::AssertUnwindSafe(async {
        peer.finish()
            .await
            .map_err(|error| ConnectError::native("Join and verify native model peer", error))
    })
    .catch_unwind()
    .await;
    let outcomes = [
        ("body", body),
        ("native_cleanup", native_cleanup),
        ("original_cleanup", original_cleanup),
        ("peer_cleanup", peer_cleanup),
    ];
    if outcomes
        .iter()
        .any(|(_, outcome)| !matches!(outcome, Ok(Ok(()))))
    {
        // ModelPeer::finish also reports prior HTTP/scenario refusals after a
        // joined task. Keep that error distinct from native close evidence.
        // This fixture root was created with mode 0700. Error sources and
        // panic text may contain private request data and stay only here.
        let retained = root
            .take()
            .map(tempfile::TempDir::keep)
            .or_else(|| original.take().map(|fixture| fixture._directory.keep()));
        let recorded = retained.as_ref().is_some_and(|path| {
            use std::os::unix::fs::OpenOptionsExt;
            (|| -> io::Result<()> {
                let mut file = std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .mode(0o600)
                    .open(path.join("private-probe-failure.txt"))?;
                writeln!(
                    file,
                    "phase={phase}; model_requests={}",
                    client.0.load(Ordering::SeqCst)
                )?;
                for (owner, outcome) in &outcomes {
                    match outcome {
                        Ok(Ok(())) => writeln!(file, "{owner}=ok")?,
                        Ok(Err(error)) => writeln!(file, "{owner}=error: {error:?}")?,
                        Err(panic) => {
                            let message = panic
                                .downcast_ref::<String>()
                                .map(String::as_str)
                                .or_else(|| panic.downcast_ref::<&str>().copied())
                                .unwrap_or("non-text panic");
                            writeln!(file, "{owner}=panic: {message}")?;
                        }
                    }
                }
                file.sync_all()
            })()
            .is_ok()
        });
        let class = |outcome: &Result<Result<(), ConnectError>, Box<dyn std::any::Any + Send>>| {
            match outcome {
                Ok(Ok(())) => "ok",
                Ok(Err(_)) => "error",
                Err(_) => "panic",
            }
        };
        let _ = writeln!(
            io::stderr(),
            "private probe failed: phase={phase}; model_requests={}; body={}; native_cleanup={}; original_cleanup={}; peer_cleanup={}; private_evidence_written={recorded}; retained_root={}",
            client.0.load(Ordering::SeqCst),
            class(&outcomes[0].1),
            class(&outcomes[1].1),
            class(&outcomes[2].1),
            class(&outcomes[3].1),
            retained
                .as_ref()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "unavailable".into())
        );
    }
    drop(original);
    drop(root);
    // Body failure wins over cleanup failure. Resume an original panic only
    // after all three cleanup attempts; typed errors stay owned until here.
    // Every non-success remains a failure, even if private evidence could not
    // be written. Public output contains fixed labels rather than error data.
    for (owner, outcome) in outcomes {
        match outcome {
            Ok(Ok(())) => (),
            Ok(Err(_error)) => panic!(
                "Actual private probe control failed at {owner}; see retained private evidence"
            ),
            Err(panic) => std::panic::resume_unwind(panic),
        }
    }
}
