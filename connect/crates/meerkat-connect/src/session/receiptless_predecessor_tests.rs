//! Read genuine ce43 predecessor snapshots with the selected current runtime.
//! Every write is made to a fresh copy. This proves nondirected R5 receipt
//! settlement only: no consent owner, G association or physical adoption.
use super::*;
use futures_util::FutureExt;
use meerkat_core::{RunId, lifecycle::InputId, session_hosting::HostingClaim};
use meerkat_runtime::{
    ColdPredecessorSettlement, LogicalRuntimeId, PredecessorOwnerLoss,
    PredecessorSettlementRefusal, RuntimeDriverError, RuntimeSessionPersistenceProfile,
    RuntimeStore, SettledBy,
    completion::{CompletionClass, CompletionOutcome},
    input_state::{InputLifecycleState, InputTerminalOutcome, StoredInputState},
    terminal_status::{
        InputTerminalReceipt, InputTerminalReceiptObservation, InputTerminalReceiptRead,
        InputTerminalReceiptScope, InteractionSelector, TerminalWitnessSource,
    },
};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    panic::{AssertUnwindSafe, resume_unwind},
    path::{Component, Path, PathBuf},
    time::Duration,
};

const STAGED: &str = "CONNECT_R5_RECEIPTLESS_STAGED";
const CONSUMED: &str = "CONNECT_R5_RECEIPTLESS_CONSUMED";
const OUTPUT: &str = "CONNECT_R5_RECEIPTLESS_OUTPUT";
const SETTLE_BOUND: Duration = Duration::from_secs(120);
const SCOPE: &str =
    "nondirected predecessor receipt only; no consent, G, physical or adoption proof";

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct FilePin {
    sha256: String,
    size: u64,
    mode: u32,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Fixture {
    schema: String,
    session: SessionId,
    input: InputId,
    run: RunId,
    snapshots: BTreeMap<String, BTreeMap<String, FilePin>>,
}
#[derive(Deserialize)]
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

fn root(env: &str) -> PathBuf {
    let path =
        PathBuf::from(std::env::var_os(env).expect("explicit retained snapshot root required"));
    assert!(path.is_absolute());
    assert_eq!(
        path.canonicalize().unwrap(),
        path,
        "snapshot root must be canonical"
    );
    assert!(fs::symlink_metadata(&path).unwrap().is_dir());
    path
}
fn inventory(root: &Path) -> BTreeMap<String, FilePin> {
    fn walk(root: &Path, dir: &Path, files: &mut BTreeMap<String, FilePin>) {
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            let metadata = fs::symlink_metadata(&path).unwrap();
            assert!(
                !metadata.file_type().is_symlink(),
                "snapshot symlink refused"
            );
            if metadata.is_dir() {
                walk(root, &path, files);
                continue;
            }
            assert!(
                metadata.is_file() && metadata.nlink() == 1,
                "only independent regular files"
            );
            let relative = path
                .strip_prefix(root)
                .unwrap()
                .to_str()
                .unwrap()
                .to_owned();
            let bytes = fs::read(&path).unwrap();
            assert!(
                files
                    .insert(
                        relative,
                        FilePin {
                            sha256: format!("{:x}", Sha256::digest(&bytes)),
                            size: bytes.len() as u64,
                            mode: metadata.mode() & 0o777
                        }
                    )
                    .is_none()
            );
        }
    }
    let mut files = BTreeMap::new();
    walk(root, root, &mut files);
    files
}
fn exact_copy(source: &Path, destination: &Path, expected: &BTreeMap<String, FilePin>) {
    assert_eq!(
        &inventory(source),
        expected,
        "entire immutable source snapshot must match"
    );
    fs::create_dir(destination).unwrap();
    for (relative, pin) in expected {
        assert!(
            Path::new(relative)
                .components()
                .all(|c| matches!(c, Component::Normal(_)))
        );
        let bytes = fs::read(source.join(relative)).unwrap();
        assert_eq!(bytes.len() as u64, pin.size);
        assert_eq!(format!("{:x}", Sha256::digest(&bytes)), pin.sha256);
        let path = destination.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(pin.mode)
            .open(&path)
            .unwrap();
        file.write_all(&bytes).unwrap();
        file.set_permissions(fs::Permissions::from_mode(pin.mode))
            .unwrap();
        file.sync_all().unwrap();
    }
    assert_eq!(&inventory(source), expected, "source changed during copy");
    assert_eq!(
        &inventory(destination),
        expected,
        "copy must precede all native opening"
    );
}
fn retain(root: &Path, name: &str, value: &Value) -> std::io::Result<()> {
    let bytes = serde_json::to_vec_pretty(value)?;
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(root.join(name))?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    fs::File::open(root)?.sync_all()
}
fn row_json(rows: &[StoredInputState]) -> Vec<Value> {
    rows.iter()
        .map(|row| serde_json::to_value(row).unwrap())
        .collect()
}
fn no_receipt(row: &Value) {
    for field in [
        "terminal_completion",
        "interaction_terminal_outbox",
        "terminal_completion_unavailable",
    ] {
        assert!(
            !row.as_object().unwrap().contains_key(field),
            "old fixture unexpectedly has {field}"
        );
    }
}
fn tail_json(
    tail: &meerkat_core::persistence_contract::HeadCanonicalProvisionalTailAuthority,
) -> Value {
    json!({"authority_version":tail.authority_version(), "session_id":tail.session_id(),
        "base_store_revision":tail.base_store_revision(), "base_committed_head_token":tail.base_committed_head_token(),
        "physical_store_revision":tail.physical_store_revision(), "physical_head_token":tail.physical_head_token(),
        "run_id":tail.run_id(), "candidate_sequence":tail.candidate_sequence()})
}
fn receipt_json(receipt: &InputTerminalReceipt) -> Value {
    let scope = match receipt.scope() {
        InputTerminalReceiptScope::Run { run_id } => json!({"type":"run", "run_id":run_id}),
        InputTerminalReceiptScope::RuntimeTermination => json!({"type":"runtime_termination"}),
        _ => panic!("unreviewed native receipt scope"),
    };
    json!({"input_id":receipt.input_id(), "terminal":receipt.terminal(),
        "attempt_count":receipt.attempt_count(), "scope":scope, "run_id":receipt.run_id(),
        "owner_input_id":receipt.owner_input_id(), "recipients":receipt.recipient_input_ids(),
        "outcome":receipt.outcome()})
}
fn settled_by(by: SettledBy) -> &'static str {
    match by {
        SettledBy::TailConsumed => "tail_consumed",
        SettledBy::Ended => "ended",
        SettledBy::PendingTerminalConverged => "pending_terminal_converged",
        SettledBy::OutcomeUnknown => "outcome_unknown",
        _ => panic!("unreviewed native settlement kind"),
    }
}
fn settlement_json(result: &ColdPredecessorSettlement) -> Value {
    // Debug below is diagnostic only; scope and outcome use typed projections.
    json!({"session":result.session_id, "tail_diagnostic":format!("{:?}",result.tail),
        "owner_loss_diagnostic":format!("{:?}",result.owner_loss),
        "inputs":result.inputs.iter().map(|input| json!({"input_id":input.input_id,
            "settled_by":settled_by(input.settled_by), "interaction_id":input.interaction_id,
            "published_terminal_seq":input.published_terminal_seq,
            "receipt":receipt_json(&input.receipt)})).collect::<Vec<_>>(),
        "durable_rows_closed":result.durable_rows_closed, "terminals_owed":result.terminals_owed,
        "migrated_without_receipt":result.migrated_without_receipt})
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
        // No assertions or awaits between creating the owners and placing them
        // in the caller's outer cleanup slot. No session/actor is materialized.
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
    async fn absent(&self, id: &SessionId) {
        assert!(
            self.machine
                .current_session_registration_witness(id)
                .await
                .is_none()
        );
        assert!(
            self.machine
                .current_executor_attachment_witness(id)
                .await
                .is_none()
        );
    }
    async fn settle(
        &self,
        id: &SessionId,
        claim: &HostingClaim,
        output: &Path,
        phase: &str,
    ) -> ColdPredecessorSettlement {
        let result = self
            .service
            .settle_cold_predecessor_before(
                id,
                claim,
                meerkat_core::time_compat::Instant::now() + SETTLE_BOUND,
            )
            .await;
        match result {
            Ok(value) => {
                retain(output, &format!("{phase}.json"), &settlement_json(&value)).unwrap();
                value
            }
            Err(error) => {
                // A refusal's progress is not a closing census. Preserve it,
                // fail this attempt and join the same owners; never resubmit.
                let recorded = retain(
                    output,
                    &format!("{phase}-refusal.json"),
                    &json!({
                    "diagnostic":error.to_string(), "progress_diagnostic":format!("{:?}",error.progress), "complete":false}),
                );
                panic!("native settlement refused: {error}; evidence write: {recorded:?}");
            }
        }
    }
    async fn receipt(&self, fixture: &Fixture) -> InputTerminalReceipt {
        let read = self
            .machine
            .input_terminal_receipt(
                &fixture.session,
                InteractionSelector::InputId(fixture.input.clone()),
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(read.source, TerminalWitnessSource::DurableStore);
        let InputTerminalReceiptRead::Finalized(receipt) = read.report else {
            panic!("receipt not finalized")
        };
        let observed = self
            .machine
            .input_terminal_receipt_observation(
                &fixture.session,
                InteractionSelector::InputId(fixture.input.clone()),
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(observed.source, TerminalWitnessSource::DurableStore);
        let InputTerminalReceiptObservation::Finalized(summary) = observed.report else {
            panic!("summary not finalized")
        };
        assert_eq!(summary.input_id(), receipt.input_id());
        assert_eq!(summary.terminal(), receipt.terminal());
        assert_eq!(summary.attempt_count(), receipt.attempt_count());
        assert_eq!(summary.scope(), receipt.scope());
        assert_eq!(summary.run_id(), receipt.run_id());
        assert_eq!(summary.owner_input_id(), receipt.owner_input_id());
        assert_eq!(summary.recipient_input_ids(), receipt.recipient_input_ids());
        assert_eq!(summary.class(), receipt.outcome().class());
        *receipt
    }
    async fn close(&self, id: &SessionId, claim: Option<&HostingClaim>) -> (Vec<String>, Value) {
        let mut issues = Vec::new();
        let mut machine_task_terminal = false;
        let mut unpublished_lease_released = false;
        let mut registration_removed = false;
        if let Some(claim) = claim {
            // Service shutdown joins actors, not the machine's independent R5
            // publication/removal slot. Begin first awaits that exact slot,
            // then only reads/checks the claim and census. Drop a successful
            // never-published lease immediately: no tail drive, sweep, finish,
            // registration or receipt repair occurs in this cleanup barrier.
            let mut stalled = false;
            loop {
                let waited = AssertUnwindSafe(self.machine.begin_predecessor_settlement_before(
                    claim,
                    meerkat_core::time_compat::Instant::now() + SETTLE_BOUND,
                ))
                .catch_unwind()
                .await;
                match waited {
                    Ok(Ok(lease)) => {
                        machine_task_terminal = true;
                        drop(lease);
                        unpublished_lease_released = true;
                        break;
                    }
                    Ok(Err(PredecessorSettlementRefusal::Runtime(
                        RuntimeDriverError::RuntimeTerminalPublicationInProgress { .. },
                    ))) => {
                        if !stalled {
                            issues.push("R5 task exceeded a cleanup observation deadline".into());
                            stalled = true;
                        }
                        // Retain the same claim/machine/service and continue
                        // observing until the task ends. The outer command
                        // owner retains the process at its total test bound.
                    }
                    Ok(Err(error)) => {
                        // Every other begin refusal follows the slot wait in
                        // this pinned native implementation. It cannot grant
                        // success, but the prior task no longer runs.
                        machine_task_terminal = true;
                        issues.push(format!("post-join R5 begin check refused: {error}"));
                        break;
                    }
                    Err(_) => {
                        if !stalled {
                            issues.push("R5 cleanup observation panicked".into());
                            stalled = true;
                        }
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                }
            }
        }
        let unregister = AssertUnwindSafe(async {
            if let Some(witness) = self.machine.current_session_registration_witness(id).await {
                // Cleanup can remove a leftover exact registration, but cannot
                // turn a failed observation into a successful reader result.
                self.machine
                    .unregister_session_registration_until_terminal_if_current(&witness)
                    .await
                    .map_err(|error| error.to_string())?;
                registration_removed = true;
                return Err("unexpected registration required cleanup".to_owned());
            }
            Ok(())
        })
        .catch_unwind()
        .await;
        let registration_check_completed =
            matches!(&unregister, Ok(Ok(()))) || registration_removed;
        match unregister {
            Ok(Ok(())) => {}
            Ok(Err(error)) => issues.push(error),
            Err(_) => issues.push("unregister panicked".into()),
        }
        // Always attempt service actor shutdown after machine cleanup, including
        // when exact registration removal failed or panicked.
        let shutdown = AssertUnwindSafe(self.service.try_shutdown())
            .catch_unwind()
            .await;
        let service_shutdown_completed = matches!(&shutdown, Ok(Ok(())));
        match shutdown {
            Ok(Ok(())) => {}
            Ok(Err(error)) => issues.push(error.to_string()),
            Err(_) => issues.push("service shutdown panicked".into()),
        }
        let evidence = json!({"machine_settlement_task_observed_terminal":machine_task_terminal,
            "fresh_unpublished_lease_released":unpublished_lease_released,
            "registration_check_completed":registration_check_completed,
            "unexpected_registration_removed":registration_removed,
            "service_actor_shutdown_completed":service_shutdown_completed,
            "original_claim_retained_through_cleanup":claim.is_some(), "errors":issues});
        (issues, evidence)
    }
}
fn complete(result: &ColdPredecessorSettlement, fixture: &Fixture) {
    assert_eq!(result.session_id, fixture.session);
    assert_eq!(result.durable_rows_closed, 1);
    assert!(result.terminals_owed.is_empty());
    assert!(result.migrated_without_receipt.is_empty());
    assert!(
        matches!(
            result.owner_loss,
            Some(PredecessorOwnerLoss::NoConsentOwner)
        ),
        "this fixture has no issuing owner and cannot certify consent loss settlement"
    );
}
fn exact_receipt(receipt: &InputTerminalReceipt, fixture: &Fixture, consumed: bool) {
    assert_eq!(receipt.input_id(), &fixture.input);
    assert_eq!(receipt.owner_input_id(), &fixture.input);
    assert_eq!(
        receipt.recipient_input_ids(),
        std::slice::from_ref(&fixture.input)
    );
    assert_eq!(receipt.terminal(), &InputTerminalOutcome::Consumed);
    assert_eq!(receipt.attempt_count(), 1);
    if consumed {
        assert_eq!(
            receipt.scope(),
            &InputTerminalReceiptScope::RuntimeTermination
        );
        assert!(receipt.run_id().is_none());
        assert_eq!(receipt.outcome().class(), CompletionClass::OutcomeUnknown);
        assert!(
            matches!(receipt.outcome(), CompletionOutcome::OutcomeUnknown { run_id } if run_id == &fixture.run),
            "unknown must carry the actual old consumed run, not the settlement's runless scope"
        );
    } else {
        assert_eq!(
            receipt.scope(),
            &InputTerminalReceiptScope::Run {
                run_id: fixture.run.clone()
            }
        );
        assert_eq!(receipt.run_id(), Some(&fixture.run));
        assert_eq!(
            receipt.outcome().class(),
            CompletionClass::CompletedWithoutResult
        );
        assert!(matches!(
            receipt.outcome(),
            CompletionOutcome::CompletedWithoutResult
        ));
    }
}
async fn case(label: &str, source: &Path, output: &Path, fixture: &Fixture) {
    let expected = &fixture.snapshots[label];
    let copy = output.join(format!("{label}-store"));
    exact_copy(source, &copy, expected);
    let evidence = output.join(label);
    fs::create_dir(&evidence).unwrap();
    let capture: Capture =
        serde_json::from_slice(&fs::read(copy.join("CAPTURE.json")).unwrap()).unwrap();
    assert_eq!(capture.session, fixture.session);
    assert_eq!(capture.input, fixture.input);
    assert_eq!(capture.run, fixture.run);
    assert_eq!(capture.model_requests, 1);
    assert_eq!(capture.rows.len(), 1);
    no_receipt(&capture.rows[0]);
    let consumed = label == "consumed";
    let old_recovered: Option<Value> = consumed
        .then(|| serde_json::from_slice(&fs::read(copy.join("RECOVERED.json")).unwrap()).unwrap());
    let mut owner: Option<Native> = None;
    let mut held_claim: Option<HostingClaim> = None;
    let caught = AssertUnwindSafe(async {
        owner = Some(Native::open(&copy).await);
        let native = owner.as_ref().unwrap();
        held_claim = Some(
            native
                .machine
                .grant_session_hosting(&fixture.session)
                .unwrap(),
        );
        let claim = held_claim.as_ref().unwrap();
        assert!(claim.is_cross_process());
        assert_eq!(claim.session_id(), &fixture.session);
        assert_eq!(claim.owner(), native.machine.hosting_authority().owner());
        native.absent(&fixture.session).await;
        let runtime_id = LogicalRuntimeId::for_session(&fixture.session);
        let before = native.rows(&fixture.session).await;
        assert_eq!(before.len(), 1);
        assert_eq!(before[0].state.input_id, fixture.input);
        assert_eq!(before[0].seed.last_run_id.as_ref(), Some(&fixture.run));
        let before_json = row_json(&before);
        no_receipt(&before_json[0]);
        let tail = native
            .store
            .load_head_canonical_provisional_tail(&runtime_id)
            .await
            .unwrap();
        if let Some(old) = &old_recovered {
            assert_eq!(old["session"], json!(fixture.session));
            assert_eq!(old["input"], json!(fixture.input));
            assert_eq!(old["run"], json!(fixture.run));
            assert_eq!(json!(before_json), old["after"]);
            assert_eq!(before[0].seed.phase, InputLifecycleState::Consumed);
            assert_eq!(
                before[0].seed.terminal_outcome,
                Some(InputTerminalOutcome::Consumed)
            );
            assert!(before[0].state.persisted_input.is_none());
            assert!(tail.is_none());
            let sequence = before[0].seed.last_boundary_sequence.unwrap();
            assert_eq!(json!(sequence), old["boundary_sequence"]);
            let boundary = native
                .store
                .load_boundary_receipt(&runtime_id, &fixture.run, sequence)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(
                serde_json::to_value(boundary).unwrap(),
                old["boundary_receipt"]
            );
        } else {
            assert_eq!(before_json, capture.rows);
            assert_eq!(before[0].seed.phase, InputLifecycleState::Staged);
            assert!(before[0].seed.terminal_outcome.is_none());
            assert!(before[0].seed.last_boundary_sequence.is_none());
            assert_eq!(tail_json(&tail.unwrap()), capture.provisional_tail);
            let boundaries = native
                .store
                .load_committed_boundary_receipts(&runtime_id, &fixture.run)
                .await
                .unwrap();
            assert_eq!(
                serde_json::to_value(boundaries).unwrap(),
                capture.committed_boundaries
            );
        }
        retain(
            &evidence,
            "before.json",
            &json!({"rows":before_json,"same_claim":true,"scope":SCOPE}),
        )
        .unwrap();
        let first = native
            .settle(&fixture.session, claim, &evidence, "first")
            .await;
        complete(&first, fixture);
        assert_eq!(first.inputs.len(), 1);
        let input = &first.inputs[0];
        assert_eq!(input.input_id, fixture.input);
        assert!(input.interaction_id.is_none() && input.published_terminal_seq.is_none());
        assert_eq!(
            input.settled_by,
            if consumed {
                SettledBy::OutcomeUnknown
            } else {
                SettledBy::TailConsumed
            }
        );
        exact_receipt(&input.receipt, fixture, consumed);
        if !consumed {
            assert!(first.tail.is_some());
        }
        if let Some(tail) = &first.tail {
            assert_eq!(tail.run_id, fixture.run);
            assert_eq!(tail.boundary_sequence, 1);
        }
        native.absent(&fixture.session).await;
        let receipt = native.receipt(fixture).await;
        exact_receipt(&receipt, fixture, consumed);
        assert_eq!(receipt_json(&receipt), receipt_json(&input.receipt));
        let after = native.rows(&fixture.session).await;
        assert_eq!(after.len(), 1);
        assert_eq!(after[0].state.input_id, fixture.input);
        assert_eq!(after[0].seed.last_run_id.as_ref(), Some(&fixture.run));
        assert_eq!(after[0].seed.phase, InputLifecycleState::Consumed);
        assert_eq!(
            after[0].seed.terminal_outcome,
            Some(InputTerminalOutcome::Consumed)
        );
        assert_eq!(after[0].seed.last_boundary_sequence, Some(1));
        assert!(after[0].state.persisted_input.is_none());
        let after_json = row_json(&after);
        let boundary = native
            .store
            .load_boundary_receipt(&runtime_id, &fixture.run, 1)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(boundary.run_id, fixture.run);
        assert_eq!(boundary.sequence, 1);
        assert_eq!(boundary.contributing_input_ids, vec![fixture.input.clone()]);
        let boundary_json = serde_json::to_value(boundary).unwrap();
        if let Some(old) = &old_recovered {
            assert_eq!(boundary_json, old["boundary_receipt"]);
        }
        retain(&evidence, "boundary.json", &boundary_json).unwrap();
        retain(
            &evidence,
            "after.json",
            &json!({"rows":after_json,"receipt":receipt_json(&receipt)}),
        )
        .unwrap();
        // Same service, machine and original live claim. No second grant, no
        // registration, no fresh input and no replay after OutcomeUnknown.
        let repeat = native
            .settle(&fixture.session, claim, &evidence, "repeat")
            .await;
        complete(&repeat, fixture);
        assert!(
            repeat.inputs.is_empty(),
            "repeat must perform no new convergence"
        );
        assert_eq!(row_json(&native.rows(&fixture.session).await), after_json);
        let repeated_receipt = native.receipt(fixture).await;
        exact_receipt(&repeated_receipt, fixture, consumed);
        assert_eq!(receipt_json(&repeated_receipt), receipt_json(&receipt));
        native.absent(&fixture.session).await;
        let repeated_boundary = native
            .store
            .load_boundary_receipt(&runtime_id, &fixture.run, 1)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            serde_json::to_value(repeated_boundary).unwrap(),
            boundary_json
        );
        retain(
            &evidence,
            "repeat-census.json",
            &json!({"rows":after_json,"receipt":receipt_json(&repeated_receipt),
            "same_original_claim":true,"owner_loss":"no_consent_owner","scope":SCOPE}),
        )
        .unwrap();
    })
    .catch_unwind()
    .await;
    let (mut issues, cleanup) = if let Some(native) = &owner {
        native.close(&fixture.session, held_claim.as_ref()).await
    } else {
        (Vec::new(), json!({"native_owner_created":false}))
    };
    // Hold the original claim across both settlement calls and shutdown.
    drop(held_claim);
    drop(owner);
    if std::panic::catch_unwind(AssertUnwindSafe(|| {
        assert_eq!(&inventory(source), expected)
    }))
    .is_err()
    {
        issues.push("immutable original changed or could not be rechecked".into());
    }
    let success = caught.is_ok() && issues.is_empty();
    let written = retain(
        &evidence,
        "RESULT.json",
        &json!({"status":if success {"passed"} else {"failed"},
        "body_completed":caught.is_ok(),"body_failure":caught.as_ref().err().map(|error| {
            error.downcast_ref::<String>().cloned().or_else(|| error.downcast_ref::<&str>().map(|s|(*s).to_owned()))
                .unwrap_or_else(|| "non-string panic; original payload is rethrown".into())
        }),"cleanup":cleanup,"cleanup_issues":issues,"scope":SCOPE,
        "session":fixture.session,"input":fixture.input,"historical_run":fixture.run,
        "consent_completed":false,"G_adoption":false,"original":source,"writable_copy":copy}),
    );
    // A report-write failure must not replace the first assertion/native error.
    if let Err(panic) = caught {
        if let Err(error) = &written {
            let _ = writeln!(
                std::io::stderr().lock(),
                "reader evidence write failed: {error}"
            );
        }
        resume_unwind(panic);
    }
    written.unwrap();
    assert!(
        issues.is_empty(),
        "retained owner cleanup or immutable snapshot check failed: {issues:?}"
    );
}

#[tokio::test]
#[ignore = "requires the exact retained ce43 paired snapshots and a fresh writable output root"]
async fn current_r5_reads_genuine_ce43_staged_and_receiptless_consumed() {
    let fixture: Fixture =
        serde_json::from_str(include_str!("receiptless_predecessor_fixture.json")).unwrap();
    assert_eq!(
        fixture.schema,
        "genuine-ce43-paired-receiptless-predecessor-v1"
    );
    let staged = root(STAGED);
    let consumed = root(CONSUMED);
    assert_ne!(staged, consumed);
    let output =
        PathBuf::from(std::env::var_os(OUTPUT).expect("fresh caller-owned output required"));
    assert!(output.is_absolute());
    let parent = output.parent().unwrap();
    assert_eq!(parent.canonicalize().unwrap(), parent);
    assert!(!output.exists());
    for source in [&staged, &consumed] {
        assert!(!output.starts_with(source) && !source.starts_with(&output));
    }
    fs::create_dir(&output).unwrap();
    fs::set_permissions(&output, fs::Permissions::from_mode(0o700)).unwrap();
    case("staged", &staged, &output, &fixture).await;
    case("consumed", &consumed, &output, &fixture).await;
    assert_eq!(inventory(&staged), fixture.snapshots["staged"]);
    assert_eq!(inventory(&consumed), fixture.snapshots["consumed"]);
    retain(
        &output,
        "RESULT.json",
        &json!({"status":"passed","cases":["staged","consumed"],
        "session":fixture.session,"input":fixture.input,"historical_run":fixture.run,
        "new_input_admission":false,"provider_execution":false,"scope":SCOPE}),
    )
    .unwrap();
}
