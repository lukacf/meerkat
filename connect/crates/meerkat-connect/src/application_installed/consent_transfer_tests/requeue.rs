//! Native lifecycle control for the same installed owner. The socket exists
//! only in the explicitly selected libtest child. It supplies no approval,
//! delivery receipt, work context or native input. Those come from the real
//! Console, issuer and the gateway's single R6 delivery owner.
use super::*;
use futures_util::FutureExt;
use meerkat_core::{
    ApprovalId, ApprovalRecord, ApprovalStatus, SessionId,
    approval::{ApprovalConsentCustody, ApprovalConsentDeliveryState, ApprovalConsentTransition},
    lifecycle::InputId,
};
use meerkat_mob::{AgentIdentity, MemberIdentityManagement, MemberRunStarts};
use meerkat_runtime::{
    MeerkatMachine, RuntimeSessionRegistrationWitness,
    input_state::{
        InputAbandonReason, InputLifecycleState, InputTerminalOutcome, StoredInputState,
    },
    meerkat_machine::UnregisterQueuedInputs,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::panic::AssertUnwindSafe;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::UnixListener,
    sync::oneshot,
};

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Request {
    command: String,
    approval_id: ApprovalId,
}
pub(super) struct Held {
    record: ApprovalRecord,
    session: SessionId,
    registration: RuntimeSessionRegistrationWitness,
    requeued: Option<StoredInputState>,
}
impl Held {
    pub(super) fn captured(&self) -> Option<(&ApprovalRecord, &SessionId, &StoredInputState)> {
        self.requeued
            .as_ref()
            .map(|row| (&self.record, &self.session, row))
    }
}
pub(super) struct Server {
    stop: oneshot::Sender<()>,
    join: tokio::task::JoinHandle<Result<(), String>>,
}
fn require(value: bool, reason: &str) -> Result<(), String> {
    if value {
        Ok(())
    } else {
        Err(reason.to_owned())
    }
}
fn err(error: impl std::fmt::Display) -> String {
    error.to_string()
}
impl Server {
    pub(super) fn start(control: Arc<Control>) -> Result<Self, String> {
        // The isolated fixture TMPDIR can exceed sockaddr_un's pathname limit.
        // Keep this private socket owner in a fresh short directory, not ambient TMPDIR.
        let socket_root = if cfg!(target_os = "macos") {
            "/private/tmp"
        } else {
            "/tmp"
        };
        let directory = tempfile::Builder::new()
            .prefix("consent-rq-")
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir_in(socket_root)
            .map_err(err)?;
        let socket = directory.path().join("owner.sock");
        let listener = UnixListener::bind(&socket).map_err(err)?;
        save(
            &control.directory.join("requeue-socket.json"),
            &json!({"path":socket}),
        );
        let (stop, mut stopped) = oneshot::channel();
        let join = tokio::spawn(async move {
            let _directory = directory;
            let mut held = None;
            let mut completed = false;
            let mut sequence = 0;
            loop {
                let (mut stream, _) = tokio::select! {
                    _ = &mut stopped => return require(completed, "native Requeue sequence did not complete"),
                    accepted = listener.accept() => accepted.map_err(err)?,
                };
                let outcome = AssertUnwindSafe(async {
                    require(!completed, "one native lifecycle sequence only")?;
                    let mut raw = Vec::new();
                    tokio::time::timeout(
                        Duration::from_secs(10),
                        (&mut stream).take(16_385).read_to_end(&mut raw),
                    )
                    .await
                    .map_err(err)?
                    .map_err(err)?;
                    require(raw.len() <= 16_384, "bounded control request")?;
                    let request: Request = serde_json::from_slice(&raw).map_err(err)?;
                    let result = tokio::time::timeout(
                        Duration::from_secs(60),
                        execute(&control, &request, &mut held),
                    )
                    .await
                    .map_err(err)??;
                    Ok::<_, String>((request, result))
                })
                .catch_unwind()
                .await;
                sequence += 1;
                let result = match outcome {
                    Ok(Ok((request, result))) => {
                        completed = request.command == "resume";
                        json!({"status":"passed","request":request,"native":result})
                    }
                    Ok(Err(error)) => json!({"status":"failed","error":error}),
                    Err(_) => {
                        json!({"status":"failed","error":"native lifecycle control panicked"})
                    }
                };
                save(
                    &control.directory.join(format!("requeue-{sequence}.json")),
                    &result,
                );
                let response = serde_json::to_vec(&result).map_err(err)?;
                require(response.len() <= 1_048_576, "bounded control response")?;
                let sent =
                    tokio::time::timeout(Duration::from_secs(10), stream.write_all(&response))
                        .await
                        .map_err(err)?
                        .map_err(err);
                if result["status"] != "passed" {
                    return Err(result.to_string());
                }
                sent?;
                if control.capture && result["request"]["command"] == "capture" {
                    // The response and durable snapshot exist before the real
                    // process exits. No Rust disposal/withdrawal is substituted.
                    std::process::exit(77);
                }
            }
        });
        Ok(Self { stop, join })
    }
    pub(super) async fn finish(self) -> Result<(), String> {
        let _ = self.stop.send(());
        self.join.await.map_err(err)?
    }
}
async fn selected_input(
    machine: &MeerkatMachine,
    session: &SessionId,
    input: &InputId,
) -> Result<StoredInputState, String> {
    let observation = machine.session_input_states(session).await.map_err(err)?;
    let mut rows = observation
        .report
        .into_iter()
        .filter(|row| &row.state.input_id == input);
    let row = rows.next().ok_or("exact native input absent")?;
    require(rows.next().is_none(), "duplicate native input")?;
    Ok(row)
}
// Fixed-size, best-effort fixture evidence. It does not decide lifecycle truth.
fn record_requeue_observation(control: &Control, row: &StoredInputState, closed: Option<bool>) {
    let terminal = match row.seed.terminal_outcome.as_ref() {
        None => Value::Null,
        Some(InputTerminalOutcome::Consumed) => json!({"outcome_type":"consumed"}),
        Some(InputTerminalOutcome::Superseded { .. }) => json!({"outcome_type":"superseded"}),
        Some(InputTerminalOutcome::Coalesced { .. }) => json!({"outcome_type":"coalesced"}),
        Some(InputTerminalOutcome::Abandoned { reason }) => {
            let reason = match reason {
                InputAbandonReason::Retired => "retired",
                InputAbandonReason::Reset => "reset",
                InputAbandonReason::Stopped => "stopped",
                InputAbandonReason::Destroyed => "destroyed",
                InputAbandonReason::Cancelled => "cancelled",
                InputAbandonReason::MaxAttemptsExhausted { .. } => "max_attempts_exhausted",
                InputAbandonReason::NeverExecuted => "never_executed",
                InputAbandonReason::ToolProcessInterrupted => "tool_process_interrupted",
                InputAbandonReason::RequeueExpired => "requeue_expired",
                InputAbandonReason::ResumeRefused => "resume_refused",
                _ => "other",
            };
            json!({"outcome_type":"abandoned","reason":reason})
        }
        Some(_) => json!({"outcome_type":"other"}),
    };
    let (stage, filename) = if closed.is_some() {
        ("after_unregister", "requeue-after-observation.json")
    } else {
        ("before_unregister", "requeue-before-observation.json")
    };
    let observation = json!({
        "schema":"case59-requeue-observation-v1", "stage":stage,
        "seed":{"phase":row.seed.phase, "terminal_outcome":terminal,
            "recovery_lane":row.seed.recovery_lane},
        "state":{"stop_requeue":row.state.stop_requeue},
        "terminal_details_omitted":true,
        "requested_unregister":{"queued_inputs":"requeue", "max_age_ms":300_000},
        "unregister_returned_current":closed,
        // Joining a prior unregister retains its original disposition. The
        // selected public method returns only bool, not that disposition.
        "actual_unregister_disposition":null,
        "actual_unregister_disposition_observation":"not_exposed_by_selected_public_api",
    });
    let written = (|| -> std::io::Result<()> {
        let bytes = serde_json::to_vec(&observation).map_err(std::io::Error::other)?;
        if bytes.len() > 4096 {
            return Err(std::io::Error::other(
                "bounded requeue observation exceeded",
            ));
        }
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(control.directory.join(filename))?;
        file.write_all(&bytes)?;
        file.sync_all()
    })();
    if written.is_err() {
        // Diagnostic failure must not replace the original assertion/error.
        let _ = std::io::Write::write_all(
            &mut std::io::stderr(),
            b"case59_requeue_observation_write_failed\n",
        );
    }
}
async fn execute(
    control: &Control,
    request: &Request,
    held: &mut Option<Held>,
) -> Result<Value, String> {
    // These are clones of the exact production owners observed in bind_runtime.
    // No native owner slot is rebound and no extra HostingClaim masks custody.
    let runtime = control
        .runtime
        .lock()
        .map_err(err)?
        .clone()
        .ok_or("runtime not bound")?;
    let service = runtime
        .session_service()
        .ok_or("actual session service absent")?;
    let machine = service
        .acquire_runtime_adapter(None)
        .map_err(err)?
        .ok_or("actual machine absent")?;
    let owner = control
        .owner
        .lock()
        .map_err(err)?
        .clone()
        .ok_or("issuer not selected")?;
    let record = owner.approvals().get(&request.approval_id).map_err(err)?;
    let consent = record.consent.as_ref().ok_or("scoped consent absent")?;
    let scope = consent.scope.as_ref().ok_or("consent scope absent")?;
    require(
        consent.withdrawal.is_none()
            && consent.consumed_at.is_none()
            && consent.custody == Some(ApprovalConsentCustody::Parked),
        "unspent parked candidate required",
    )?;
    let session = scope.session_id.clone();
    let handle = runtime.handle();
    require(
        !handle.is_public_owner(),
        "control must not retain public native ownership",
    )?;
    // This closed fixture has no identity-intent writer. Observe the real
    // canonical posture rather than assuming Stop disables reconciliation.
    // This is not a reservation against arbitrary external host adoption.
    let management = handle
        .identity_management(&AgentIdentity::from("original"))
        .await
        .map_err(err)?;
    require(
        management == MemberIdentityManagement::CommandDriven,
        "selected original must remain command-driven",
    )?;
    let roster = handle.roster().await;
    let original = roster
        .get(&AgentIdentity::from("original"))
        .ok_or("actual original absent")?;
    require(
        original.bridge_session_id() == Some(&session),
        "canonical original/session mismatch",
    )?;
    require(
        !control.capture || request.command != "resume",
        "capture cannot resume the old owner",
    )?;
    match request.command.as_str() {
        "hold" => {
            require(
                held.is_none() && record.status == ApprovalStatus::Pending,
                "initial pending request required",
            )?;
            let report = handle.stop().await.map_err(err)?;
            let original = report
                .members
                .get(&AgentIdentity::from("original"))
                .ok_or("original missing from Stop report")?;
            require(
                matches!(original.starts, MemberRunStarts::Held),
                "original run starts were not held",
            )?;
            require(
                report.not_holdable().next().is_none(),
                "a member was not held",
            )?;
            let registration = machine
                .current_session_registration_witness(&session)
                .await
                .ok_or("current native registration absent")?;
            let result = json!({"stop":report,"identity_management":format!("{management:?}"),"session_id":session,"registration":format!("{registration:?}"),
                "approval":record});
            *held = Some(Held {
                record,
                session,
                registration,
                requeued: None,
            });
            Ok(result)
        }
        "requeue" => {
            let prior = held.as_mut().ok_or("Stop has not completed")?;
            require(
                prior.requeued.is_none()
                    && prior.record.approval_id == request.approval_id
                    && prior.session == session
                    && prior.record.proposed_action == record.proposed_action
                    && record.status == ApprovalStatus::Approved,
                "exact approved successor required",
            )?;
            let mut deliveries = consent
                .deliveries
                .iter()
                .filter(|d| d.transition == ApprovalConsentTransition::Approved);
            let delivery = deliveries.next().ok_or("approved notice absent")?;
            require(deliveries.next().is_none(), "duplicate approved notice")?;
            let ApprovalConsentDeliveryState::Admitted {
                session_id,
                input_id,
            } = &delivery.state
            else {
                return Err("R6 must settle its actual admitted notice receipt".into());
            };
            require(session_id == &session, "notice destination mismatch")?;
            let input: InputId = serde_json::from_value(json!(input_id)).map_err(err)?;
            let before = selected_input(&machine, &session, &input).await?;
            record_requeue_observation(control, &before, None);
            require(
                before.seed.phase == InputLifecycleState::Queued
                    && before.seed.terminal_outcome.is_none()
                    && before.state.stop_requeue.is_none(),
                "notice must be queued before actual Requeue",
            )?;
            require(
                machine
                    .current_session_registration_witness(&session)
                    .await
                    .as_ref()
                    == Some(&prior.registration),
                "native registration changed before Requeue",
            )?;
            let closed = machine
                .unregister_session_registration_until_terminal_with_queued_inputs_if_current(
                    &prior.registration,
                    UnregisterQueuedInputs::Requeue {
                        max_age: Duration::from_secs(300),
                    },
                )
                .await
                .map_err(err)?;
            require(
                closed
                    && machine
                        .current_session_registration_witness(&session)
                        .await
                        .is_none(),
                "exact unregister did not join to absence",
            )?;
            let after = selected_input(&machine, &session, &input).await?;
            record_requeue_observation(control, &after, Some(closed));
            require(
                after.seed.phase == InputLifecycleState::Queued
                    && after.seed.terminal_outcome.is_none()
                    && after
                        .state
                        .stop_requeue
                        .is_some_and(|mark| mark.max_age_ms == 300_000),
                "native owner did not retain the exact notice with its Requeue mark",
            )?;
            require(
                owner.approvals().get(&request.approval_id).map_err(err)? == record,
                "Requeue changed or withdrew parked consent",
            )?;
            let result = json!({"session_id":session,"identity_management":format!("{management:?}"),"input_id":input,"before":before,"after":after,
                "registration":format!("{:?}",prior.registration),"approval":record});
            prior.requeued = Some(after);
            Ok(result)
        }
        "resume" => {
            let prior = held.as_ref().ok_or("Stop has not completed")?;
            let input = &prior
                .requeued
                .as_ref()
                .ok_or("Requeue has not completed")?
                .state
                .input_id;
            require(
                prior.record.approval_id == request.approval_id && prior.session == session,
                "resume request differs",
            )?;
            let registration_before_resume =
                machine.current_session_registration_witness(&session).await;
            require(
                registration_before_resume.is_none(),
                "registration returned before explicit Resume",
            )?;
            // Actual Core explicit resume owns sidecar/actor/materialization
            // replacement. Meta, its identity bridge, machine and host remain.
            handle.resume().await.map_err(err)?;
            let current = machine
                .current_session_registration_witness(&session)
                .await
                .ok_or("resume registered no successor")?;
            require(
                current != prior.registration,
                "resume reused a retired registration",
            )?;
            require(
                machine
                    .current_executor_attachment_witness(&session)
                    .await
                    .is_some(),
                "resume published no actual executor",
            )?;
            let roster = handle.roster().await;
            require(
                roster
                    .get(&AgentIdentity::from("original"))
                    .and_then(|r| r.bridge_session_id())
                    == Some(&session),
                "actual mob resume replaced the session",
            )?;
            // The exact predecessor's cleanup authority cannot retire its
            // successor. A false result is native evidence, not a local flag.
            require(
                !machine
                    .unregister_session_registration_until_terminal_with_queued_inputs_if_current(
                        &prior.registration,
                        UnregisterQueuedInputs::Requeue {
                            max_age: Duration::from_secs(300),
                        },
                    )
                    .await
                    .map_err(err)?,
                "stale predecessor retired the successor",
            )?;
            require(
                machine
                    .current_session_registration_witness(&session)
                    .await
                    .as_ref()
                    == Some(&current),
                "successor changed during stale cleanup",
            )?;
            Ok(
                json!({"session_id":session,"identity_management":format!("{management:?}"),
                "registration_absent_before_resume":registration_before_resume.is_none(),"input_id":input,"predecessor":format!("{:?}",prior.registration),
                "successor":format!("{current:?}"),"input":selected_input(&machine,&session,input).await?,
                "approval":owner.approvals().get(&request.approval_id).map_err(err)?}),
            )
        }
        "capture" => {
            require(
                control.capture,
                "capture is selected only by its distinct ignored process control",
            )?;
            super::cold_capture::capture(
                control,
                held.as_ref().ok_or("Requeue was not observed")?,
                &machine,
                &record,
                &management,
            )
            .await
        }
        _ => Err("unknown lifecycle command".into()),
    }
}
