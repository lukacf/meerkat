//! Current recovered-boundary receipt proof from one genuinely captured same-G tail.
use super::*;
use crate::application_consent::{
    ApplicationConsentOwner, ConsentApplicationIdentity, ConsentIssuerSelection,
};
use futures_util::FutureExt;
use meerkat_core::{RunId, approval::ApprovalListFilter, lifecycle::InputId};
use meerkat_runtime::{
    LogicalRuntimeId, RuntimeSessionPersistenceProfile, RuntimeStore,
    input_state::{InputLifecycleState, InputTerminalOutcome, StoredInputState},
    recovery::{
        DurableTailDriveRole, DurableTailRecoveryOutcome, recover_durable_tail_under_hosting_claim,
    },
    terminal_status::InteractionSelector,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Read, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    panic::{AssertUnwindSafe, resume_unwind},
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
const ROOT: &str = "CONNECT_CURRENT_RECEIPTFUL_G_RECOVERY_ROOT";
const READY: &str = "CONNECT_CURRENT_RECEIPTFUL_G_RECOVERED";
const SELECTOR: &str =
    "session::current_g_boundary_recovery::recover_same_g_receiptful_boundary_child";
const BOUND: Duration = Duration::from_secs(120);
fn hash(raw: &[u8]) -> String {
    format!("{:x}", Sha256::digest(raw))
}
fn read(path: &Path) -> Vec<u8> {
    let meta = fs::symlink_metadata(path).unwrap();
    assert!(
        meta.is_file() && meta.nlink() == 1,
        "exact private regular fixture file required"
    );
    fs::read(path).unwrap()
}
fn value(path: &Path) -> Value {
    serde_json::from_slice(&read(path)).unwrap()
}
fn save(root: &Path, name: &str, data: &Value) {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(root.join(name))
        .unwrap();
    file.write_all(&serde_json::to_vec_pretty(data).unwrap())
        .unwrap();
    file.sync_all().unwrap();
    fs::File::open(root).unwrap().sync_all().unwrap();
}
fn selected(selection: &Value) -> Vec<u8> {
    let path = Path::new(selection["path"].as_str().unwrap());
    assert!(path.is_absolute());
    let bytes = read(path);
    assert_eq!(hash(&bytes), selection["sha256"]);
    bytes
}
fn row_json(rows: &[StoredInputState]) -> Vec<Value> {
    rows.iter()
        .map(|row| serde_json::to_value(row).unwrap())
        .collect()
}
fn no_receipt(row: &Value) {
    for key in [
        "terminal_completion",
        "interaction_terminal_outbox",
        "terminal_completion_unavailable",
    ] {
        assert!(
            !row.as_object().unwrap().contains_key(key),
            "captured input unexpectedly carries {key}"
        );
    }
}
fn tail_json(
    t: &meerkat_core::persistence_contract::HeadCanonicalProvisionalTailAuthority,
) -> Value {
    json!({"authority_version":t.authority_version(),"session_id":t.session_id(),
        "base_store_revision":t.base_store_revision(),"base_committed_head_token":t.base_committed_head_token(),
        "physical_store_revision":t.physical_store_revision(),"physical_head_token":t.physical_head_token(),
        "run_id":t.run_id(),"candidate_sequence":t.candidate_sequence()})
}
fn check_envelope(before: &Value, after: &Value) {
    let mut left = before.as_object().unwrap().clone();
    let mut right = after.as_object().unwrap().clone();
    for key in [
        "current_state",
        "terminal_outcome",
        "recovery_lane",
        "last_run_id",
        "last_boundary_sequence",
        "persisted_input",
        "terminal_completion",
    ] {
        left.remove(key);
        right.remove(key);
    }
    assert_eq!(
        left, right,
        "current native recovery changed non-envelope facts"
    );
}
struct Native {
    service: Arc<PersistentSessionService<FactoryAgentBuilder>>,
    machine: Arc<MeerkatMachine>,
    store: Arc<dyn RuntimeStore>,
}
impl Native {
    async fn open(realm: &crate::application_setup::ApplicationSetupRealm) -> Self {
        let mut startup = SessionStartup::open(realm.resolve().unwrap())
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
        // No await or assertion after constructing the owned service and before returning it.
        Self {
            service: Arc::new(service),
            machine,
            store,
        }
    }
}
fn same_association(capture: &Value) {
    assert_eq!(
        serde_json::from_slice::<Value>(&selected(&capture["association"])).unwrap(),
        capture["association_document"]
    );
    selected(&capture["association_document"]["resources"]["handoff"]);
    assert_eq!(
        capture["association_document"]["projected_peer"],
        capture["projected_peer"]
    );
    assert_eq!(
        capture["association_document"]["physical"],
        capture["physical"]
    );
    assert_eq!(
        capture["association_document"]["session"],
        capture["session_id"]
    );
}
fn inventory(
    root: &Path,
) -> (
    std::collections::BTreeSet<PathBuf>,
    std::collections::BTreeSet<PathBuf>,
) {
    let mut files = std::collections::BTreeSet::new();
    let mut dirs = std::collections::BTreeSet::from([root.to_owned()]);
    let mut pending = vec![root.to_owned()];
    while let Some(dir) = pending.pop() {
        assert!(fs::symlink_metadata(&dir).unwrap().is_dir());
        for entry in fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            let meta = fs::symlink_metadata(&path).unwrap();
            if meta.is_dir() {
                dirs.insert(path.clone());
                pending.push(path);
            } else {
                assert!(meta.is_file() && meta.nlink() == 1);
                files.insert(path);
            }
        }
    }
    (files, dirs)
}
fn check_snapshot(snapshot: &Value, originals: bool) {
    for key in if originals {
        &[("original", "original_root"), ("copy", "copy_root")][..]
    } else {
        &[("copy", "copy_root")][..]
    } {
        let expected_files: std::collections::BTreeSet<_> = snapshot["files"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| PathBuf::from(r[key.0].as_str().unwrap()))
            .collect();
        let expected_dirs: std::collections::BTreeSet<_> = snapshot["directories"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| PathBuf::from(r[key.0].as_str().unwrap()))
            .collect();
        let mut files = std::collections::BTreeSet::new();
        let mut dirs = std::collections::BTreeSet::new();
        for roots in snapshot["roots"].as_array().unwrap() {
            let (f, d) = inventory(Path::new(roots[key.1].as_str().unwrap()));
            files.extend(f);
            dirs.extend(d);
        }
        assert_eq!(files, expected_files);
        assert_eq!(dirs, expected_dirs);
    }
    for row in snapshot["directories"].as_array().unwrap() {
        assert_eq!(
            fs::symlink_metadata(row["copy"].as_str().unwrap())
                .unwrap()
                .mode()
                & 0o777,
            row["mode"]
        );
        if originals {
            assert_eq!(
                fs::symlink_metadata(row["original"].as_str().unwrap())
                    .unwrap()
                    .mode()
                    & 0o777,
                row["mode"]
            );
        }
    }
    for row in snapshot["files"].as_array().unwrap() {
        let copy = Path::new(row["copy"].as_str().unwrap());
        let bytes = read(copy);
        assert_eq!(bytes.len() as u64, row["bytes"]);
        assert_eq!(hash(&bytes), row["sha256"]);
        assert_eq!(fs::metadata(copy).unwrap().mode() & 0o777, row["mode"]);
        if originals {
            let original = Path::new(row["original"].as_str().unwrap());
            assert_eq!(
                read(original),
                bytes,
                "fixture changed before recovery admission"
            );
            assert_eq!(fs::metadata(original).unwrap().mode() & 0o777, row["mode"]);
        }
    }
}
#[tokio::test]
#[ignore = "private same-G current receiptful recovery phase; only the retained parent may select it"]
async fn recover_same_g_receiptful_boundary_child() {
    let root = PathBuf::from(std::env::var_os(ROOT).expect("explicit retained parent selection"))
        .canonicalize()
        .unwrap();
    let selected_binary = value(&root.join("result.json"))["candidate"]["emitted"].clone();
    assert_eq!(
        hash(&fs::read(std::env::current_exe().unwrap()).unwrap()),
        selected_binary["sha256"]
    );
    let capture = value(&root.join("g-owner/g-capture.json"));
    let staged_files = value(&root.join("STAGED-FILES.json"));
    check_snapshot(&staged_files, true);
    same_association(&capture);
    let session: SessionId = serde_json::from_value(capture["session_id"].clone()).unwrap();
    let input: InputId = serde_json::from_value(capture["origin_input_id"].clone()).unwrap();
    let run: RunId = serde_json::from_value(capture["origin_run_id"].clone()).unwrap();
    let realm = serde_json::from_value(capture["realm"].clone()).unwrap();
    let issuer: ConsentIssuerSelection = serde_json::from_value(capture["issuer"].clone()).unwrap();
    let identity: ConsentApplicationIdentity =
        serde_json::from_value(capture["identity"].clone()).unwrap();
    let owner = ApplicationConsentOwner::reopen(issuer, &identity).unwrap();
    let approvals = owner
        .approvals()
        .list(ApprovalListFilter::default())
        .unwrap();
    assert_eq!(
        serde_json::to_value(&approvals).unwrap(),
        capture["approvals"]
    );
    assert_eq!(approvals.len(), 1);
    let approval = &approvals[0];
    assert_eq!(
        serde_json::to_value(approval).unwrap(),
        capture["selected_approval"]
    );
    let consent = approval.consent.as_ref().unwrap();
    let scope = consent.scope.as_ref().unwrap();
    let origin = consent.origin.as_ref().unwrap();
    assert_eq!(scope.session_id, session);
    assert_eq!(origin.retained_work.run_id(), &run);
    assert_eq!(origin.retained_work.contributors().len(), 1);
    assert_eq!(origin.retained_work.contributors()[0].input_id, input);
    let association = serde_json::from_value(capture["association"].clone()).unwrap();
    let closed = crate::approval::ClosedLocalApprovalsArtifact::read_for_association(
        &association,
        Path::new(
            capture["association_document"]["approval_source"]
                .as_str()
                .unwrap(),
        ),
        &session,
    )
    .unwrap();
    closed.check().unwrap();
    assert_eq!(
        serde_json::to_value(closed.artifact()).unwrap(),
        capture["presenter_closed"]
    );
    let physical: crate::resources::PhysicalSelection =
        serde_json::from_value(capture["physical"].clone()).unwrap();
    let token = physical.seal_no_entry().unwrap();
    assert!(token.census()["operations"].as_array().unwrap().is_empty());
    let members = token.census()["members"].as_array().unwrap();
    assert_eq!(members.len(), 1);
    assert_eq!(members[0]["session"], capture["session_id"]);
    assert_eq!(members[0]["member"]["member"], "planning-lead");
    let mut native = None;
    let caught: Result<(), _> = AssertUnwindSafe(async {
        native = Some(Native::open(&realm).await);
        let n = native.as_ref().unwrap();
        let claim = n.machine.grant_session_hosting(&session).unwrap();
        assert!(claim.is_cross_process()); assert_eq!(claim.owner(), n.machine.hosting_authority().owner());
        assert_eq!(claim.domain_ref().as_ref(), Some(&scope.domain));
        assert!(n.machine.current_session_registration_witness(&session).await.is_none());
        assert!(n.machine.current_executor_attachment_witness(&session).await.is_none());
        let runtime_id = LogicalRuntimeId::for_session(&session);
        let before = n.store.load_input_states_strict(&runtime_id).await.unwrap(); assert_eq!(before.len(), 2);
        let chosen: Vec<_> = before.iter().filter(|row| row.state.input_id == input).collect(); assert_eq!(chosen.len(), 1);
        let row = chosen[0]; assert_eq!(row.seed.phase, InputLifecycleState::Staged);
        assert!(row.seed.terminal_outcome.is_none()); assert_eq!(row.seed.last_run_id.as_ref(), Some(&run));
        assert!(matches!(row.state.persisted_input.as_ref(), Some(Input::Prompt(_))),
            "the actual governed G context-transfer producer submits a Prompt, not a tracked Interaction");
        let before_json = row_json(&before); let selected_before = serde_json::to_value(row).unwrap(); no_receipt(&selected_before);
        let live_ids: std::collections::HashSet<_> = capture["native_inputs"].as_array().unwrap().iter()
            .map(|r| r["input_id"].as_str().unwrap()).collect();
        let ids: std::collections::HashSet<_> = before_json.iter().map(|r| r["input_id"].as_str().unwrap()).collect();
        assert_eq!(ids, live_ids, "actual durable census differs from the original live G");
        let tail = n.store.load_head_canonical_provisional_tail(&runtime_id).await.unwrap().unwrap();
        assert_eq!(tail.session_id(), &session); assert_eq!(tail.run_id(), &run);
        let boundaries_before = n.store.load_committed_boundary_receipts(&runtime_id, &run).await.unwrap();
        save(&root, "STAGED-NATIVE.json", &json!({"session":session,"input":input,"run":run,
            "rows":before_json,"provisional_tail":tail_json(&tail),"boundaries":boundaries_before,
            "original_files":staged_files,"association":association,"approval_census":approvals,
            "scope":"same real G before current recovery; no R5 result"}));
        // Await this one entered operation directly. No timeout drops/retries it.
        // Parent owns the exact process; any forced cleanup is a sticky failure.
        let (outcome, whole_blob) = recover_durable_tail_under_hosting_claim(n.store.as_ref(), &session,
            &n.machine, &claim, DurableTailDriveRole::Host).await.unwrap();
        assert!(whole_blob.is_none());
        let DurableTailRecoveryOutcome::Committed { disposition, boundary_sequence, recovered } = outcome
            else { panic!("the selected current helper did not commit this captured G tail: {outcome:?}") };
        assert_eq!(recovered.id(), &session);
        assert!(matches!(disposition, meerkat_runtime::recovery::DurableTailRecoveryDisposition::CommitCompleted
            | meerkat_runtime::recovery::DurableTailRecoveryDisposition::CommitCompletedRetainInputs),
            "the held completed G tail must commit its completed boundary");
        let after = n.store.load_input_states_strict(&runtime_id).await.unwrap(); assert_eq!(after.len(), before.len());
        let after_json = row_json(&after);
        let chosen: Vec<_> = after.iter().filter(|row| row.state.input_id == input).collect(); assert_eq!(chosen.len(), 1);
        let row = chosen[0]; assert_eq!(row.seed.phase, InputLifecycleState::Consumed);
        assert_eq!(row.seed.terminal_outcome, Some(InputTerminalOutcome::Consumed));
        assert_eq!(row.seed.last_run_id.as_ref(), Some(&run)); assert_eq!(row.seed.last_boundary_sequence, Some(boundary_sequence));
        assert!(row.seed.recovery_lane.is_none());
        let selected_after = serde_json::to_value(row).unwrap();
        let completion = &selected_after["terminal_completion"];
        assert_eq!(completion["input_id"], json!(input));
        assert_eq!(completion["owner_input_id"], json!(input));
        assert_eq!(completion["batch_ordinal"], 0);
        assert_eq!(completion["batch_key"], json!({"scope":"run","run_id":run}));
        assert_eq!(completion["phase"], json!({"phase":"pending"}));
        assert_eq!(completion["candidate"], json!({"candidate_type":"completed_without_result"}));
        assert_eq!(completion["completion_input_ids"], json!([input]));
        assert_eq!(completion["requires_session_checkpoint"], false);
        for key in ["candidate_digest", "completion_input_ids_digest"] {
            assert!(completion[key].as_str().is_some_and(|value| !value.is_empty()));
        }
        assert!(completion.get("outcome").is_none());
        assert!(selected_after.get("interaction_terminal_outbox").is_none());
        assert!(selected_after.get("terminal_completion_unavailable").is_none());
        check_envelope(&selected_before, &selected_after);
        assert_eq!(selected_after["persisted_input"], selected_before["persisted_input"],
            "pending terminal completion must retain the actual original Prompt until its drain");
        for old in before_json.iter().filter(|r| *r != &selected_before) {
            assert!(after_json.contains(old), "current recovery changed another native input");
        }
        let receipt = n.store.load_boundary_receipt(&runtime_id, &run, boundary_sequence).await.unwrap().unwrap();
        assert_eq!(receipt.run_id, run); assert_eq!(receipt.sequence, boundary_sequence);
        assert_eq!(receipt.contributing_input_ids, vec![input.clone()]);
        let boundaries_after = n.store.load_committed_boundary_receipts(&runtime_id, &run).await.unwrap();
        let old_json = serde_json::to_value(&boundaries_before).unwrap();
        let new_json = serde_json::to_value(&boundaries_after).unwrap();
        assert_eq!(boundaries_after.len(), boundaries_before.len()+1);
        for old in old_json.as_array().unwrap() { assert!(new_json.as_array().unwrap().contains(old)); }
        let observed = n.machine.input_terminal_receipt_observation(&session,
            InteractionSelector::InputId(input.clone())).await.unwrap().expect("actual pending receipt");
        assert_eq!(observed.source, meerkat_runtime::terminal_status::TerminalWitnessSource::DurableStore);
        assert!(matches!(&observed.report,
            meerkat_runtime::terminal_status::InputTerminalReceiptObservation::Pending {
                input_id, phase: InputLifecycleState::Consumed,
                terminal: Some(InputTerminalOutcome::Consumed), last_run_id: Some(observed_run), ..
            } if input_id == &input && observed_run == &run));
        assert_eq!(row_json(&n.store.load_input_states_strict(&runtime_id).await.unwrap()), after_json);
        assert!(n.machine.current_session_registration_witness(&session).await.is_none());
        assert!(n.machine.current_executor_attachment_witness(&session).await.is_none());
        assert_eq!(owner.approvals().list(ApprovalListFilter::default()).unwrap(), approvals);
        same_association(&capture); closed.check().unwrap(); check_snapshot(&staged_files, false);
        save(&root, "RECOVERED.json", &json!({"status":"same_G_current_boundary_receipt_pending",
            "session":session,"input":input,"run":run,"boundary_sequence":boundary_sequence,
            "before":before_json,"after":after_json,"boundary_receipt":receipt,"pending_receipt_observed":true,
            "recovery_disposition":format!("{disposition:?}"),"association":association,"identity":identity,
            "issuer":owner.selection(),"approvals":approvals,"presenter_closed":closed.artifact(),
            "physical":token.selection(),"physical_census":token.census(),"cross_process_claim":true,
            "registration_absent":true,"executor_absent":true,"current_R5":null,"fresh_G_work":null}));
        println!("{READY}"); std::io::stdout().flush().unwrap();
        let mut byte=[0]; std::io::stdin().read_exact(&mut byte).unwrap(); assert_eq!(byte,[1]);
        let _retained = (claim, recovered, &owner, &token, &closed);
        std::process::exit(72);
    }).catch_unwind().await;
    let cleanup = if let Some(n) = native.as_ref() {
        AssertUnwindSafe(n.service.try_shutdown())
            .catch_unwind()
            .await
    } else {
        Ok(Ok(()))
    };
    let diagnostic = std::panic::catch_unwind(AssertUnwindSafe(|| {
        save(
            &root,
            "RECOVERY-FAILED.json",
            &json!({
        "body_panicked":caught.is_err(),"service_shutdown_ok":matches!(cleanup,Ok(Ok(()))),
        "service_shutdown_error":cleanup.as_ref().ok().and_then(|result|result.as_ref().err()).map(ToString::to_string),
        "service_shutdown_panicked":cleanup.is_err(),
        "scope":"failed capture; no current settlement or adoption qualification"}),
        )
    }));
    if let Err(panic) = caught {
        resume_unwind(panic);
    }
    if let Err(panic) = cleanup {
        resume_unwind(panic);
    }
    cleanup.unwrap().unwrap();
    diagnostic.unwrap();
    panic!("current G recovery returned without its exact exit");
}
fn copy_tree(source: &Path, target: &Path, rows: &mut Vec<Value>, dirs: &mut Vec<Value>) {
    let meta = fs::symlink_metadata(source).unwrap();
    assert!(meta.is_dir());
    fs::create_dir(target).unwrap();
    fs::set_permissions(target, fs::Permissions::from_mode(meta.mode() & 0o777)).unwrap();
    dirs.push(json!({"original":source,"copy":target,"mode":meta.mode() & 0o777}));
    let mut entries: Vec<_> = fs::read_dir(source).unwrap().map(|e| e.unwrap()).collect();
    entries.sort_by_key(|e| e.file_name());
    let names: Vec<_> = entries.iter().map(|e| e.file_name()).collect();
    for entry in entries {
        let from = entry.path();
        let to = target.join(entry.file_name());
        let before = fs::symlink_metadata(&from).unwrap();
        if before.is_dir() {
            copy_tree(&from, &to, rows, dirs);
            continue;
        }
        assert!(
            before.is_file() && before.nlink() == 1,
            "snapshot refuses links or special nodes"
        );
        let bytes = read(&from);
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(before.mode() & 0o777)
            .open(&to)
            .unwrap();
        file.write_all(&bytes).unwrap();
        file.sync_all().unwrap();
        fs::set_permissions(&to, fs::Permissions::from_mode(before.mode() & 0o777)).unwrap();
        file.sync_all().unwrap();
        let after = fs::symlink_metadata(&from).unwrap();
        assert_eq!(
            (before.dev(), before.ino(), before.mode(), before.len()),
            (after.dev(), after.ino(), after.mode(), after.len())
        );
        assert_eq!(read(&from), bytes);
        assert_eq!(read(&to), bytes);
        rows.push(json!({"original":from,"copy":to,"sha256":hash(&bytes),"bytes":bytes.len(),"mode":before.mode() & 0o777}));
    }
    let mut after_names: Vec<_> = fs::read_dir(source)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    after_names.sort();
    assert_eq!(after_names, names);
    let after = fs::symlink_metadata(source).unwrap();
    assert_eq!(
        (after.dev(), after.ino(), after.mode()),
        (meta.dev(), meta.ino(), meta.mode())
    );
    fs::File::open(target).unwrap().sync_all().unwrap();
}
fn snapshot(output: &Path, phase: &str) {
    let report = value(&output.join("result.json"));
    assert_eq!(report["status"], "captured_predecessor");
    let cases = report["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 1);
    let case = PathBuf::from(cases[0]["case_directory"].as_str().unwrap());
    assert_eq!(case, output.join("cases/current-g-held-predecessor"));
    let root = output.join(format!("snapshot-{phase}"));
    fs::create_dir(&root).unwrap();
    let mut rows = Vec::new();
    let mut dirs = Vec::new();
    copy_tree(&case, &root.join("case"), &mut rows, &mut dirs);
    copy_tree(
        &output.join("g-owner"),
        &root.join("g-owner"),
        &mut rows,
        &mut dirs,
    );
    let data = json!({"phase":phase,"files":rows,"directories":dirs,
        "roots":[{"original_root":case,"copy_root":root.join("case")},
            {"original_root":output.join("g-owner"),"copy_root":root.join("g-owner")}],
        "scope":"whole original case and G owner bytes, no relocation/adoption authority"});
    check_snapshot(&data, true);
    save(output, &format!("{phase}-FILES.json"), &data);
}
pub(crate) async fn recover_parent(output: &Path) {
    snapshot(output, "STAGED");
    let stderr = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(output.join("recover.stderr"))
        .unwrap();
    let executable = std::env::current_exe().unwrap().canonicalize().unwrap();
    let executable_hash = hash(&fs::read(&executable).unwrap());
    assert_eq!(
        executable_hash,
        value(&output.join("result.json"))["candidate"]["emitted"]["sha256"]
    );
    let mut child = tokio::process::Command::new(&executable)
        .args([
            "--exact",
            SELECTOR,
            "--ignored",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(ROOT, output)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(stderr)
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let pid = child.id();
    let mut observed = Vec::new();
    let mut status = None;
    let caught = AssertUnwindSafe(async {
        assert!(pid.is_some());
        let mut lines = tokio::io::BufReader::new(child.stdout.take().unwrap()).lines();
        tokio::time::timeout(BOUND, async {
            loop {
                let line = lines
                    .next_line()
                    .await
                    .unwrap()
                    .expect("current G recovery exited before durable result");
                assert!(line.len() <= 1024 * 1024 && observed.len() < 1024);
                let ready = line.ends_with(READY);
                observed.push(line);
                if ready {
                    break;
                }
            }
        })
        .await
        .expect("current G recovery evidence deadline");
        assert_eq!(
            value(&output.join("RECOVERED.json"))["status"],
            "same_G_current_boundary_receipt_pending"
        );
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
                .expect("current G recovery exit deadline")
                .unwrap(),
        );
    })
    .catch_unwind()
    .await;
    let mut errors = Vec::new();
    let mut forced = false;
    let mut reap_failures = 0u64;
    if status.is_none() {
        match child.try_wait() {
            Ok(Some(exit)) => status = Some(exit),
            other => {
                forced = true;
                errors.push(format!("forced cleanup after {other:?}"));
                if let Err(e) = child.start_kill() {
                    errors.push(e.to_string());
                }
                loop {
                    match tokio::time::timeout(Duration::from_secs(30), child.wait()).await {
                        Ok(Ok(exit)) => {
                            status = Some(exit);
                            break;
                        }
                        other => {
                            reap_failures = reap_failures.saturating_add(1);
                            if errors.len() < 16 {
                                errors.push(format!("retained child reap refused: {other:?}"));
                            }
                            tokio::time::sleep(Duration::from_millis(100)).await;
                        }
                    }
                }
            }
        }
    }
    let success = caught.is_ok()
        && !forced
        && errors.is_empty()
        && status.as_ref().and_then(|s| s.code()) == Some(72);
    let retained = std::panic::catch_unwind(AssertUnwindSafe(|| {
        save(
            output,
            "RECOVER-PROCESS.json",
            &json!({
        "executable":executable,"executable_sha256":executable_hash,"selector":SELECTOR,"pid":pid,"stdout":observed,
        "exit_code":status.as_ref().and_then(|s|s.code()),"confirmed_reaped":status.is_some(),
        "forced_cleanup":forced,"cleanup_errors":errors,"reap_observation_failures":reap_failures,"phase_succeeded":success}),
        )
    }));
    if let Err(panic) = caught {
        resume_unwind(panic);
    }
    retained.unwrap();
    assert!(success, "current G recovery failed");
    assert_eq!(hash(&fs::read(&executable).unwrap()), executable_hash);
    snapshot(output, "CONSUMED");
    check_snapshot(&value(&output.join("STAGED-FILES.json")), false);
    let capture = value(&output.join("g-owner/g-capture.json"));
    let recovered = value(&output.join("RECOVERED.json"));
    assert_eq!(capture["session_id"], recovered["session"]);
    assert_eq!(capture["origin_input_id"], recovered["input"]);
    assert_eq!(capture["origin_run_id"], recovered["run"]);
    assert_eq!(capture["association"], recovered["association"]);
    let evidence: Vec<_> = [
        "result.json",
        "g-owner/g-capture.json",
        "g-owner/native-command.json",
        "STAGED-FILES.json",
        "STAGED-NATIVE.json",
        "RECOVERED.json",
        "RECOVER-PROCESS.json",
        "CONSUMED-FILES.json",
    ]
    .into_iter()
    .map(|name| {
        let path = output.join(name);
        let bytes = read(&path);
        json!({"path":path,"sha256":hash(&bytes),"bytes":bytes.len()})
    })
    .collect();
    save(
        output,
        "PRODUCER-COMPLETE.json",
        &json!({"status":"current_same_G_receiptful_fixture",
        "evidence":evidence,"executable_sha256":executable_hash,
        "first_child_exit":78,"second_child_exit":72,"source":"explicit current same-tuple receiptful acceptance composition",
        "capture":"g-owner/g-capture.json","staged":"STAGED-NATIVE.json","consumed":"RECOVERED.json",
        "scope":"real same G/native/issuer/presenter/physical producer only","current_R5":null,"full_cold_adoption":null}),
    );
}
