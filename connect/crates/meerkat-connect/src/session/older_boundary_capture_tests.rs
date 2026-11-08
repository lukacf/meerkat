//! Old ce43 producer only. Real commit-boundary crash followed by the same
//! old implementation's claim-bound recovery, never an R5 success receipt.
use super::*;
use futures_util::FutureExt;
use meerkat_client::{LlmClient, LlmDoneOutcome, LlmError, LlmEvent, LlmRequest};
use meerkat_core::{Message, RunId, lifecycle::InputId};
use meerkat_runtime::{
    LogicalRuntimeId, RuntimeSessionPersistenceProfile, RuntimeStore,
    input_state::{InputLifecycleState, InputTerminalOutcome, StoredInputState},
    recovery::{
        DurableTailDriveRole, DurableTailRecoveryOutcome, recover_durable_tail_under_hosting_claim,
    },
    terminal_status::InteractionSelector,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    os::unix::fs::OpenOptionsExt,
    panic::{AssertUnwindSafe, resume_unwind},
    path::{Path, PathBuf},
    process::Stdio,
    sync::atomic::{AtomicUsize, Ordering},
    time::Duration,
};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};

const OUTPUT: &str = "CONNECT_R5_OLDER_BOUNDARY_OUTPUT";
const CHILD_ROOT: &str = "CONNECT_R5_OLDER_BOUNDARY_CHILD_ROOT";
const READY: &str = "CONNECT_R5_OLDER_BOUNDARY_READY=";
const CAPTURE: &str = "session::older_boundary_capture_tests::older_boundary_capture_child";
const RECOVER: &str = "session::older_boundary_capture_tests::older_boundary_recover_child";
const BOUND: Duration = Duration::from_secs(120);
const ANSWER: &str = "older boundary answer";

#[derive(Default)]
struct TextClient(AtomicUsize);
#[async_trait::async_trait]
impl LlmClient for TextClient {
    fn project_replay_messages(&self, messages: &[Message]) -> Result<Vec<Message>, LlmError> {
        Ok(messages.to_vec())
    }
    fn stream<'a>(&'a self, request: &'a LlmRequest) -> meerkat_llm_core::LlmStream<'a> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Box::pin(futures_util::stream::iter(vec![
            Ok(LlmEvent::TextDelta {
                delta: ANSWER.into(),
                meta: None,
            }),
            Ok(LlmEvent::UsageUpdate {
                usage: meerkat_core::TurnUsage::host_declared(
                    meerkat_core::Provider::OpenAI,
                    &request.model,
                    meerkat_core::Usage::default(),
                ),
            }),
            Ok(LlmEvent::Done {
                outcome: LlmDoneOutcome::success(meerkat_core::StopReason::EndTurn),
            }),
        ]))
    }
    fn provider(&self) -> meerkat_core::Provider {
        meerkat_core::Provider::OpenAI
    }
    async fn health_check(&self) -> Result<(), LlmError> {
        Ok(())
    }
}

struct Native {
    service: Arc<PersistentSessionService<FactoryAgentBuilder>>,
    machine: Arc<MeerkatMachine>,
    store: Arc<dyn RuntimeStore>,
}
impl Native {
    async fn open(root: &Path) -> Self {
        let storage = crate::RealmOptions {
            context_root: root.to_owned(),
            state_root: Some(root.join("realms")),
            user_config_root: Some(root.join("home")),
        }
        .resolve("older-boundary-capture")
        .unwrap();
        let mut startup = SessionStartup::with_config(storage, Config::default())
            .await
            .unwrap();
        let store = startup.persistence().await.unwrap().runtime_store();
        assert_eq!(
            store.session_persistence_profile(),
            RuntimeSessionPersistenceProfile::HeadCanonicalV1
        );
        let SessionStartup {
            factory,
            config,
            persistence,
            ..
        } = startup;
        let (service, machine) =
            build_persistent_service_with_runtime_adapter(factory, config, 1, persistence.unwrap())
                .unwrap();
        Self {
            service: Arc::new(service),
            machine,
            store,
        }
    }
    async fn rows(&self, id: &SessionId) -> Vec<StoredInputState> {
        self.store
            .load_input_states_strict(&LogicalRuntimeId::for_session(id))
            .await
            .unwrap()
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Capture {
    session: SessionId,
    input: InputId,
    run: RunId,
    rows: Vec<Value>,
    provisional_tail: Value,
    committed_boundaries: Value,
    model_requests: usize,
}
fn retain(root: &Path, name: &str, value: &impl Serialize) {
    let bytes = serde_json::to_vec_pretty(value).unwrap();
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(root.join(name))
        .unwrap();
    file.write_all(&bytes).unwrap();
    file.sync_all().unwrap();
    std::fs::File::open(root).unwrap().sync_all().unwrap();
}
fn child_root() -> PathBuf {
    PathBuf::from(
        std::env::var_os(CHILD_ROOT).expect("only the owning parent supplies the isolated root"),
    )
    .canonicalize()
    .unwrap()
}
// The parent owns and bounds both real processes. A byte acknowledges that it
// retained the durable capture; process::exit deliberately skips Rust disposal.
fn exit_after_parent(phase: &str, code: i32) -> ! {
    println!("{READY}{phase}");
    std::io::stdout().flush().unwrap();
    let mut byte = [0];
    std::io::stdin().read_exact(&mut byte).unwrap();
    assert_eq!(byte, [1]);
    std::process::exit(code)
}
fn row_json(rows: &[StoredInputState]) -> Vec<Value> {
    rows.iter()
        .map(|row| serde_json::to_value(row).unwrap())
        .collect()
}
fn tail_observation(
    tail: &meerkat_core::persistence_contract::HeadCanonicalProvisionalTailAuthority,
) -> Value {
    json!({"authority_version":tail.authority_version(), "session_id":tail.session_id(),
        "base_store_revision":tail.base_store_revision(), "base_committed_head_token":tail.base_committed_head_token(),
        "physical_store_revision":tail.physical_store_revision(), "physical_head_token":tail.physical_head_token(),
        "run_id":tail.run_id(), "candidate_sequence":tail.candidate_sequence()})
}
fn no_receipt(row: &Value) {
    let map = row.as_object().unwrap();
    // Exact actual StoredInputState serde omits each absent field. The raw
    // store row is preserved; no private member or decoded row is rewritten.
    for key in [
        "terminal_completion",
        "interaction_terminal_outbox",
        "terminal_completion_unavailable",
    ] {
        assert!(
            !map.contains_key(key),
            "unexpected old receipt or migration field {key}"
        );
    }
}
fn unchanged_envelope(before: &Value, after: &Value) {
    let mut before = before.as_object().unwrap().clone();
    let mut after = after.as_object().unwrap().clone();
    // Exactly the fields touched by ce43 terminalize_attributed_inputs.
    for key in [
        "current_state",
        "terminal_outcome",
        "recovery_lane",
        "last_run_id",
        "last_boundary_sequence",
        "persisted_input",
    ] {
        before.remove(key);
        after.remove(key);
    }
    assert_eq!(
        after, before,
        "old production recovery changed unrelated input facts"
    );
}

#[tokio::test]
#[ignore = "private phase of the old producer; run only through its owning parent"]
async fn older_boundary_capture_child() {
    let root = child_root();
    let native = Native::open(&root).await;
    let session = Session::new();
    let id = session.id().clone();
    let client = Arc::new(TextClient::default());
    let request = CreateSessionRequest {
        model: "gpt-5.4".into(),
        prompt: String::new().into(),
        injected_context: Vec::new(),
        system_prompt: SystemPromptOverride::Inherit,
        max_tokens: None,
        event_tx: None,
        initial_turn: InitialTurnPolicy::Defer,
        deferred_prompt_policy: DeferredPromptPolicy::Discard,
        build: Some(SessionBuildOptions {
            llm_client_override: Some(meerkat::encode_llm_client_override_for_service(
                client.clone(),
            )),
            ..Default::default()
        }),
        labels: None,
    };
    let created = Box::pin(meerkat::surface::materialize_session(
        &native.service,
        &native.machine,
        session,
        request,
        {
            let service = native.service.clone();
            let machine = native.machine.clone();
            move |id| meerkat::surface::default_persistent_executor(service, machine, id)
        },
    ))
    .await
    .unwrap();
    assert_eq!(created.session_id, id);
    let claim = native.machine.grant_session_hosting(&id).unwrap();
    assert!(claim.is_cross_process());
    assert_eq!(
        claim.acquisition(),
        meerkat_core::session_hosting::HostingAcquisition::Joined,
        "the extra hold shares the real materialized session's machine lineage"
    );
    assert_eq!(claim.owner(), native.machine.hosting_authority().owner());
    let (entered, release) = native
        .machine
        .arm_runtime_loop_before_terminal_commit_test_hook(id.clone());
    let input = Input::Prompt(PromptInput::new("one real old capture input", None));
    let (accepted, completion) = native
        .machine
        .accept_input_with_completion(&id, input.clone())
        .await
        .unwrap();
    assert!(
        matches!(accepted, AcceptOutcome::Accepted { ref input_id, .. } if input_id == input.id())
    );
    let completion = completion.unwrap();
    tokio::time::timeout(BOUND, entered)
        .await
        .expect("terminal hook deadline")
        .unwrap();
    let runtime_id = LogicalRuntimeId::for_session(&id);
    let rows = native.rows(&id).await;
    assert_eq!(rows.len(), 1);
    let row = &rows[0];
    assert_eq!(&row.state.input_id, input.id());
    // The real loop persists Staged before applying the input. Its applied
    // state and terminal receipt are installed by the commit held at this hook.
    assert_eq!(
        row.seed.phase,
        InputLifecycleState::Staged,
        "old HeadCanonical input must remain durably staged before terminal commit"
    );
    assert!(row.seed.terminal_outcome.is_none());
    let tail = native
        .store
        .load_head_canonical_provisional_tail(&runtime_id)
        .await
        .unwrap()
        .unwrap();
    let run = tail.run_id().clone();
    assert_eq!(row.seed.last_run_id.as_ref(), Some(&run));
    let boundaries = native
        .store
        .load_committed_boundary_receipts(&runtime_id, &run)
        .await
        .unwrap();
    assert!(
        boundaries.is_empty(),
        "this single text turn has not committed a run boundary"
    );
    let rows = row_json(&rows);
    no_receipt(&rows[0]);
    let capture = Capture {
        session: id,
        input: input.id().clone(),
        run,
        rows,
        provisional_tail: tail_observation(&tail),
        committed_boundaries: serde_json::to_value(boundaries).unwrap(),
        model_requests: client.0.load(Ordering::SeqCst),
    };
    assert_eq!(capture.model_requests, 1);
    retain(&root, "CAPTURE.json", &capture);
    // Retain all actual owners and the hook sender through the abrupt exit.
    let _retained = (native, claim, release, completion, created, client);
    exit_after_parent("captured", 71)
}

#[tokio::test]
#[ignore = "private phase of the old producer; run only through its owning parent"]
async fn older_boundary_recover_child() {
    let root = child_root();
    let capture: Capture =
        serde_json::from_slice(&std::fs::read(root.join("CAPTURE.json")).unwrap()).unwrap();
    let native = Native::open(&root).await;
    let claim = native
        .machine
        .grant_session_hosting(&capture.session)
        .unwrap();
    assert!(claim.is_cross_process());
    assert_eq!(claim.owner(), native.machine.hosting_authority().owner());
    let runtime_id = LogicalRuntimeId::for_session(&capture.session);
    assert!(
        native
            .machine
            .current_session_registration_witness(&capture.session)
            .await
            .is_none()
    );
    assert!(
        native
            .machine
            .current_executor_attachment_witness(&capture.session)
            .await
            .is_none()
    );
    let before = native.rows(&capture.session).await;
    assert_eq!(row_json(&before), capture.rows);
    let old_tail = native
        .store
        .load_head_canonical_provisional_tail(&runtime_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(old_tail.run_id(), &capture.run);
    assert_eq!(tail_observation(&old_tail), capture.provisional_tail);
    let boundaries_before = native
        .store
        .load_committed_boundary_receipts(&runtime_id, &capture.run)
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_value(boundaries_before).unwrap(),
        capture.committed_boundaries
    );
    // This exact production helper is also called by PersistentSessionService
    // before a host materializes its actor. No input, boundary or receipt DTO
    // is supplied to recovery, and no serving registration exists here.
    let (outcome, whole_blob) = recover_durable_tail_under_hosting_claim(
        native.store.as_ref(),
        &capture.session,
        &native.machine,
        &claim,
        DurableTailDriveRole::Host,
    )
    .await
    .unwrap();
    assert!(
        whole_blob.is_none(),
        "the selected producer is HeadCanonical"
    );
    let DurableTailRecoveryOutcome::Committed {
        disposition,
        boundary_sequence,
        recovered,
    } = outcome
    else {
        panic!("old recovery did not make the required actual commit: {outcome:?}")
    };
    assert_eq!(recovered.id(), &capture.session);
    let after = native.rows(&capture.session).await;
    assert_eq!(after.len(), 1);
    let row = &after[0];
    assert_eq!(row.state.input_id, capture.input);
    assert_eq!(row.seed.phase, InputLifecycleState::Consumed);
    assert_eq!(
        row.seed.terminal_outcome,
        Some(InputTerminalOutcome::Consumed)
    );
    assert_eq!(row.seed.last_run_id.as_ref(), Some(&capture.run));
    assert_eq!(row.seed.last_boundary_sequence, Some(boundary_sequence));
    assert!(row.seed.recovery_lane.is_none());
    assert!(
        row.state.persisted_input.is_none(),
        "old nondirected terminal payload was retired by production recovery"
    );
    let after_json = row_json(&after);
    no_receipt(&after_json[0]);
    unchanged_envelope(&capture.rows[0], &after_json[0]);
    let receipt = native
        .store
        .load_boundary_receipt(&runtime_id, &capture.run, boundary_sequence)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(receipt.run_id, capture.run);
    assert_eq!(receipt.sequence, boundary_sequence);
    assert_eq!(receipt.contributing_input_ids, vec![capture.input.clone()]);
    let receipt_read = native
        .machine
        .input_terminal_receipt_observation(
            &capture.session,
            InteractionSelector::InputId(capture.input.clone()),
        )
        .await;
    let receipt_error = match receipt_read {
        Err(meerkat_runtime::RuntimeDriverError::RecoveryCorruption { reason }) => reason,
        other => panic!("old exact receipt read must expose its missing-receipt gap: {other:?}"),
    };
    assert_eq!(
        receipt_error,
        "v5 terminal input lost its exact completion receipt"
    );
    assert!(
        native
            .machine
            .current_session_registration_witness(&capture.session)
            .await
            .is_none()
    );
    assert!(
        native
            .machine
            .current_executor_attachment_witness(&capture.session)
            .await
            .is_none()
    );
    let stable = native.rows(&capture.session).await;
    assert_eq!(
        row_json(&stable),
        after_json,
        "receipt observation must not repair or mutate the row"
    );
    retain(
        &root,
        "RECOVERED.json",
        &json!({
            "status":"actual_old_recovered_boundary_without_completion", "session":capture.session,
            "input":capture.input, "run":capture.run, "boundary_sequence":boundary_sequence,
            "disposition":format!("{disposition:?}"), "before":capture.rows, "after":after_json,
            "boundary_receipt":receipt, "old_exact_receipt_error":receipt_error,
            "registration_absent":true, "executor_absent":true, "cross_process_claim":claim.is_cross_process(),
            "whole_input_census_unchanged":true,
            "scope":"nondirected old native boundary only; no issuer, physical, R5 or adoption proof"
        }),
    );
    let _retained = (native, claim, recovered);
    exit_after_parent("recovered", 72)
}

async fn phase(output: &Path, root: &Path, selector: &str, label: &str, expected: i32) {
    let stderr = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(output.join(format!("{label}.stderr")))
        .unwrap();
    let executable = std::env::current_exe().unwrap().canonicalize().unwrap();
    let mut child = tokio::process::Command::new(&executable)
        .args([
            "--exact",
            selector,
            "--ignored",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(CHILD_ROOT, root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(stderr)
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let pid = child.id().unwrap();
    let mut lines = tokio::io::BufReader::new(child.stdout.take().unwrap()).lines();
    let mut observed = Vec::new();
    let mut status = None;
    let caught = AssertUnwindSafe(async {
        tokio::time::timeout(BOUND, async {
            loop {
                let line = lines
                    .next_line()
                    .await
                    .unwrap()
                    .expect("old producer exited before its exact phase");
                assert!(
                    line.len() <= 1024 * 1024 && observed.len() < 1024,
                    "bounded fixture stdout"
                );
                let ready = line
                    .find(READY)
                    .is_some_and(|at| &line[at + READY.len()..] == label);
                observed.push(line);
                if ready {
                    break;
                }
            }
        })
        .await
        .expect("old producer readiness deadline");
        // Parent acknowledgement follows durable file validation, not merely
        // the marker. The second phase independently matches these store rows.
        let artifact = root.join(if label == "captured" {
            "CAPTURE.json"
        } else {
            "RECOVERED.json"
        });
        let _: Value = serde_json::from_slice(&std::fs::read(artifact).unwrap()).unwrap();
        tokio::time::timeout(
            Duration::from_secs(10),
            child.stdin.take().unwrap().write_all(&[1]),
        )
        .await
        .unwrap()
        .unwrap();
        status = Some(
            tokio::time::timeout(BOUND, child.wait())
                .await
                .expect("old producer exit deadline")
                .unwrap(),
        );
    })
    .catch_unwind()
    .await;
    let mut cleanup_error = None;
    let mut forced_cleanup = false;
    let mut reap_observation_failures = 0u64;
    if status.is_none() {
        match child.try_wait() {
            Ok(Some(exited)) => status = Some(exited),
            other => {
                forced_cleanup = true;
                cleanup_error = Some("phase required forced cleanup".to_owned());
                if let Err(error) = other {
                    cleanup_error = Some(error.to_string());
                }
                if let Err(error) = child.start_kill() {
                    cleanup_error = Some(error.to_string());
                }
                // A deadline is a sticky failure, never permission to drop an
                // unjoined Child. Retain the exact owner and keep observing
                // until the OS confirms terminal exit. No work is restarted.
                loop {
                    match tokio::time::timeout(Duration::from_secs(30), child.wait()).await {
                        Ok(Ok(exited)) => {
                            status = Some(exited);
                            break;
                        }
                        other => {
                            reap_observation_failures = reap_observation_failures.saturating_add(1);
                            cleanup_error
                                .get_or_insert_with(|| format!("old child reap failed: {other:?}"));
                            tokio::time::sleep(Duration::from_millis(100)).await;
                        }
                    }
                }
            }
        }
    }
    let phase_succeeded = caught.is_ok()
        && cleanup_error.is_none()
        && !forced_cleanup
        && status.as_ref().and_then(|s| s.code()) == Some(expected);

    retain(
        output,
        &format!("{label}-PROCESS.json"),
        &json!({
            "executable":executable, "selector":selector, "pid":pid, "stdout":observed,
            "exit_code":status.as_ref().and_then(|s|s.code()), "cleanup_error":cleanup_error,
            "phase_succeeded":phase_succeeded, "expected_abrupt_exit":expected,
            "forced_cleanup":forced_cleanup, "confirmed_reaped":status.is_some(),
            "reap_observation_failures":reap_observation_failures,
        }),
    );
    if let Err(panic) = caught {
        resume_unwind(panic);
    }
    assert!(cleanup_error.is_none(), "{cleanup_error:?}");
    assert_eq!(status.unwrap().code(), Some(expected));
    assert!(phase_succeeded);
}

#[tokio::test]
#[ignore = "source-bound older producer; requires a fresh retained output and exact ce43 executable"]
async fn older_ce43_capture_and_recover_consumed_without_receipt() {
    let output = PathBuf::from(
        std::env::var_os(OUTPUT).expect("root selects the fresh retained producer output"),
    );
    std::fs::create_dir(&output).expect("never reuse or overwrite an old capture");
    let output = output.canonicalize().unwrap();
    let root = output.join("realm");
    std::fs::create_dir(&root).unwrap();
    phase(&output, &root, CAPTURE, "captured", 71).await;
    phase(&output, &root, RECOVER, "recovered", 72).await;
    let capture: Capture =
        serde_json::from_slice(&std::fs::read(root.join("CAPTURE.json")).unwrap()).unwrap();
    let recovered: Value =
        serde_json::from_slice(&std::fs::read(root.join("RECOVERED.json")).unwrap()).unwrap();
    assert_eq!(
        recovered["session"],
        serde_json::to_value(capture.session).unwrap()
    );
    assert_eq!(
        recovered["input"],
        serde_json::to_value(capture.input).unwrap()
    );
    assert_eq!(recovered["run"], serde_json::to_value(capture.run).unwrap());
    retain(
        &output,
        "CAPTURE-COMPLETE.json",
        &json!({
            "status":"actual_old_native_capture_complete", "phases":2,
            "scope":"one nondirected native input; no R5/issuer/physical/resource closure or full cold adoption qualification",
            "old_core_source_expected":"ce43bec80ea866c6cb414c213463461c2a1f74a6",
            "new_r5_result":null, "source_emission_proof":"must be supplied by the external build record"
        }),
    );
}
