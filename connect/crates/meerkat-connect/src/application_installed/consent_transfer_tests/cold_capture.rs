//! Actual routable predecessor capture. The original control still resumes;
//! this distinct opt-in exits only after observing real Admitted + Requeue.
//! The captured facts grant no recovery, native completion or physical closure.
use super::*;
use meerkat_core::{
    ApprovalRecord, ApprovalStatus,
    approval::{ApprovalConsentCustody, ApprovalConsentDeliveryState, ApprovalConsentTransition},
};
use meerkat_runtime::{
    MeerkatMachine, input_state::InputLifecycleState, terminal_status::TerminalWitnessSource,
};
use serde_json::{Value, json};
use std::os::unix::fs::OpenOptionsExt;

const SELECTOR: &str = "application_installed::consent_transfer_tests::cold_capture::routable_consent_predecessor_exits_after_actual_admitted_requeue";
fn require(value: bool, message: &str) -> Result<(), String> {
    if value {
        Ok(())
    } else {
        Err(message.to_owned())
    }
}
fn durable(path: &Path, value: &Value) -> Result<(), String> {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(|e| e.to_string())?;
    file.write_all(&serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    file.sync_all().map_err(|e| e.to_string())?;
    fs::File::open(path.parent().ok_or("capture parent absent")?)
        .and_then(|f| f.sync_all())
        .map_err(|e| e.to_string())
}
pub(super) async fn capture(
    control: &Control,
    held: &super::requeue::Held,
    machine: &Arc<MeerkatMachine>,
    record: &ApprovalRecord,
    management: &meerkat_mob::MemberIdentityManagement,
) -> Result<Value, String> {
    let (pending, session, requeued) = held.captured().ok_or("actual Requeue absent")?;
    require(
        pending.approval_id == record.approval_id && record.status == ApprovalStatus::Approved,
        "capture must select the held real request",
    )?;
    let consent = record.consent.as_ref().ok_or("consent absent")?;
    let scope = consent.scope.as_ref().ok_or("scope absent")?;
    require(
        consent.routable()
            && consent.origin.is_some()
            && &scope.session_id == session
            && consent.custody == Some(ApprovalConsentCustody::Parked)
            && consent.withdrawal.is_none()
            && consent.consumed_at.is_none(),
        "actual unspent routable origin required",
    )?;
    let deliveries: Vec<_> = consent
        .deliveries
        .iter()
        .filter(|row| row.transition == ApprovalConsentTransition::Approved)
        .collect();
    require(deliveries.len() == 1, "one actual admitted notice required")?;
    let ApprovalConsentDeliveryState::Admitted {
        session_id,
        input_id,
    } = &deliveries[0].state
    else {
        return Err("native admitted notice absent".into());
    };
    let admitted_input: meerkat_core::lifecycle::InputId =
        serde_json::from_value(json!(input_id)).map_err(|e| e.to_string())?;
    require(
        session_id == session && admitted_input == requeued.state.input_id,
        "exact native admitted notice required",
    )?;
    require(
        machine
            .current_session_registration_witness(session)
            .await
            .is_none()
            && machine
                .current_executor_attachment_witness(session)
                .await
                .is_none(),
        "old exact registration/executor must be absent",
    )?;
    let owner = control
        .owner
        .lock()
        .map_err(|_| "issuer lock poisoned")?
        .clone()
        .ok_or("issuer absent")?;
    require(
        matches!(
            ApplicationConsentOwner::reopen(owner.selection().clone(), owner.identity()),
            Err(crate::application_consent::ConsentOwnerError::OwnerHeld)
        ),
        "actual old issuer exclusion must still be held",
    )?;
    let provenance = owner
        .resource_provenance(machine, session)
        .map_err(|e| e.to_string())?;
    require(
        provenance.domain() == Some(&scope.domain),
        "actual machine domain differs from issued scope",
    )?;
    let observed = machine
        .session_input_states(session)
        .await
        .map_err(|e| e.to_string())?;
    require(
        observed.source == TerminalWitnessSource::DurableStore,
        "capture requires actual durable input witnesses",
    )?;
    let rows = observed.report;
    let matched: Vec<_> = rows
        .iter()
        .filter(|row| row.state.input_id == requeued.state.input_id)
        .collect();
    require(
        matched.len() == 1
            && matched[0].seed.phase == InputLifecycleState::Queued
            && matched[0].seed.terminal_outcome.is_none()
            && serde_json::to_value(matched[0]).map_err(|e| e.to_string())?
                == serde_json::to_value(requeued).map_err(|e| e.to_string())?,
        "capture changed the exact real Requeue row",
    )?;
    let approvals = owner
        .approvals()
        .list(ApprovalListFilter::default())
        .map_err(|e| e.to_string())?;
    require(
        approvals
            .iter()
            .filter(|row| row.approval_id == record.approval_id)
            .collect::<Vec<_>>()
            == vec![record],
        "issuer census differs from selected row",
    )?;
    let bootstrap = control
        .bootstrap
        .lock()
        .map_err(|_| "bootstrap lock poisoned")?
        .clone()
        .ok_or("actual installed bootstrap absent")?;
    let mut result = json!({"schema":1,"phase":"admitted_requeued_before_exit","pid":std::process::id(),
        "session_id":session,"input_id":requeued.state.input_id,"identity_management":format!("{management:?}"),
        "registration_absent":true,"executor_absent":true,"issuer_exclusion_observed":true,"issuer":owner.selection(),"identity":owner.identity(),
        "provenance":provenance,"bootstrap":bootstrap,"native_input_source":format!("{:?}",observed.source),"selected_approval":record,"approvals":approvals,"native_inputs":rows,
        "requested_exit_code":77,"native_completion":null,"physical_disposition":null});
    // Repeat the live reads before sealing. This is a bounded observation, not
    // a reservation against a different producer. Actual death ends this owner.
    require(
        owner
            .approvals()
            .list(ApprovalListFilter::default())
            .map_err(|e| e.to_string())?
            == approvals,
        "issuer changed during capture",
    )?;
    let again = machine
        .session_input_states(session)
        .await
        .map_err(|e| e.to_string())?;
    require(
        again.source == TerminalWitnessSource::DurableStore,
        "capture recheck requires actual durable input witnesses",
    )?;
    require(
        serde_json::to_value(&again.report).map_err(|e| e.to_string())?
            == serde_json::to_value(&rows).map_err(|e| e.to_string())?
            && machine
                .current_session_registration_witness(session)
                .await
                .is_none(),
        "native rows changed during capture",
    )?;
    result["native_input_recheck_source"] = json!(format!("{:?}", again.source));
    durable(&control.directory.join("cold-capture.json"), &result)?;
    Ok(result)
}

#[test]
#[ignore = "requires exact producer tuple, SDK/resources and a fresh retained crash-capture output"]
fn routable_consent_predecessor_exits_after_actual_admitted_requeue() {
    if std::env::var_os(CHILD).is_some() {
        child();
    }
    let selected = |name| {
        let path = PathBuf::from(std::env::var_os(name).expect("explicit fixture input"));
        assert!(path.is_absolute());
        path
    };
    let output = selected("TOOLKIT_CONSENT_COLD_CAPTURE_OUTPUT");
    assert!(!output.exists());
    let Some(resources) = option_env!("MEERKAT_CONNECT_RESOURCES_SHA256") else {
        panic!("resource-bound test build");
    };
    assert_eq!(
        std::env::var("TOOLKIT_SHARED_PUBLIC_RESOURCES_SHA256").unwrap(),
        resources
    );
    let script = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../scripts/fixtures/r5_routable_predecessor.py")
        .canonicalize()
        .unwrap();
    let status = Command::new(selected("TOOLKIT_SHARED_TEST_PYTHON"))
        .args(["-I", "-B"])
        .arg(script)
        .arg("--test-executable")
        .arg(std::env::current_exe().unwrap())
        .arg("--selector")
        .arg(SELECTOR)
        .arg("--resource-executable")
        .arg(selected("TOOLKIT_SHARED_PUBLIC_EXECUTABLE"))
        .arg("--resources-sha256")
        .arg(resources)
        .arg("--toolkit-python")
        .arg(selected("TOOLKIT_LATE_ADOPTION_TOOLKIT_PYTHON"))
        .arg("--meta-python")
        .arg(selected("TOOLKIT_LATE_ADOPTION_META_PYTHON"))
        .arg("--output")
        .arg(&output)
        .status()
        .unwrap();
    assert!(
        status.success(),
        "actual predecessor capture failed: {}",
        output.display()
    );
    let report: Value =
        serde_json::from_slice(&fs::read(output.join("result.json")).unwrap()).unwrap();
    assert_eq!(report["status"], "captured_predecessor");
    let cases = report["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 1);
    use sha2::Digest;
    let pin = &cases[0]["capture"];
    let capture_path = PathBuf::from(pin["path"].as_str().unwrap());
    assert_eq!(
        capture_path,
        output.join("transfer-owner/cold-capture.json")
    );
    let bytes = fs::read(&capture_path).unwrap();
    assert_eq!(pin["bytes"], bytes.len() as u64);
    assert_eq!(pin["sha256"], format!("{:x}", sha2::Sha256::digest(&bytes)));
    let capture: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(capture["issuer_exclusion_observed"], true);
    assert_eq!(capture["native_input_source"], "DurableStore");
    assert_eq!(capture["native_input_recheck_source"], "DurableStore");
    let selection = serde_json::from_value(capture["issuer"].clone()).unwrap();
    let identity = serde_json::from_value(capture["identity"].clone()).unwrap();
    let owner = ApplicationConsentOwner::reopen(selection, &identity).unwrap();
    let rows = owner
        .approvals()
        .list(ApprovalListFilter::default())
        .unwrap();
    assert_eq!(
        serde_json::to_value(&rows).unwrap(),
        capture["approvals"],
        "real old process death must not be substituted with ordinary row withdrawal"
    );
    assert!(
        rows.iter()
            .any(|r| serde_json::to_value(r).unwrap() == capture["selected_approval"])
    );
    durable(
        &output.join("issuer-reopen.json"),
        &json!({"phase":"same_issuer_reopened_after_actual_exit",
        "issuer":owner.selection(),"identity":owner.identity(),"approvals":rows}),
    )
    .unwrap();
    // No HostingClaim is acquired, no row settlement is invoked and no new
    // physical preparation occurs. Later recovery must revalidate everything.
}
