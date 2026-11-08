//! Explicit process-only G producer. These hooks are absent outside libtest.
//! Capture does not perform R5, spend consent or prepare replacement resources.
use super::*;
use crate::{
    application_consent::{ApplicationConsentOwner, ConsentApplicationIdentity},
    application_gateway::console_consent::{ConsoleConsentRule, ConsoleConsentSelection},
    installed_service_review::composer::LiveSharedSetup,
};
use meerkat_core::{
    ApprovalId, PrincipalKind, PrincipalRef, TrustDomainId,
    approval::{
        ApprovalListFilter,
        review::{OperationReviewer, ReviewCandidate, ReviewVerdict, ReviewerFailure},
    },
};
use meerkat_toolkit::host_lifetime::control::HostLifetimeError;
use serde_json::{Value, json};
use std::{
    fs,
    io::Write,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    process::Command,
    sync::{Mutex, OnceLock, Weak},
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::UnixListener,
    sync::oneshot,
};
use toolkit_mobkit::host_gateway::ToolkitGatewayHost;
const SELECTOR: &str = "application_installed::canonical_g_capture::published_g_pending_consent_survives_actual_owner_loss";
const CHILD: &str = "CONNECT_CANONICAL_G_CHILD";
const EXIT: i32 = 78;
static CONTROL: OnceLock<Arc<Control>> = OnceLock::new();
struct Control {
    directory: PathBuf,
    origin: String,
    owner: Mutex<Option<Arc<ApplicationConsentOwner>>>,
    runtime: Arc<Mutex<Option<meerkat_mobkit::mob_handle_runtime::MobRuntime>>>,
    live: OnceLock<Weak<LiveSharedSetup>>,
    bootstrap: meerkat_toolkit::host_lifetime::control::HostBootstrapSelection,
}
fn error(e: impl std::fmt::Display) -> String {
    e.to_string()
}
fn check(ok: bool, why: &str) -> Result<(), String> {
    if ok { Ok(()) } else { Err(why.into()) }
}
fn durable(path: &Path, value: &Value) -> Result<(), String> {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(error)?;
    file.write_all(&serde_json::to_vec_pretty(value).map_err(error)?)
        .map_err(error)?;
    file.sync_all().map_err(error)?;
    fs::File::open(path.parent().ok_or("missing output parent")?)
        .and_then(|f| f.sync_all())
        .map_err(error)
}
impl toolkit_mobkit::installed_host::InstalledOperationReviewPolicy for Control {
    fn review_floor(
        &self,
        context: toolkit_mobkit::installed_host::InstalledOperationReviewContext<'_>,
    ) -> Result<
        meerkat_core::authorization::OperationReviewTier,
        meerkat_core::OperationAuthorizationError,
    > {
        use meerkat_core::{OperationAuthorizationError, authorization::OperationReviewTier};
        if !matches!(context.target(), meerkat_core::ToolOperationTarget::McpServerOperation {
            raw_operation, .. } if raw_operation == "setup_state")
        {
            return Ok(OperationReviewTier::R1);
        }
        let issuer = self
            .owner
            .lock()
            .map_err(|_| OperationAuthorizationError::Unavailable)?
            .clone()
            .ok_or(OperationAuthorizationError::Unavailable)?;
        let live = self
            .live
            .get()
            .and_then(Weak::upgrade)
            .ok_or(OperationAuthorizationError::Unavailable)?;
        live.canonical_g_review_floor(&issuer, context)
    }
}
struct NoModelReview;
#[async_trait::async_trait]
impl OperationReviewer for NoModelReview {
    async fn review(&self, _: &ReviewCandidate<'_>) -> Result<ReviewVerdict, ReviewerFailure> {
        panic!("G producer selects only its exact R3 setup_state request")
    }
}
pub(crate) fn select(
    host: &ToolkitGatewayHost,
    existing: Option<ConsoleConsentSelection>,
) -> Result<Option<ConsoleConsentSelection>, HostLifetimeError> {
    let Some(control) = CONTROL.get() else {
        return super::canonical_g_recovery::select(host, existing);
    };
    if existing.is_some() {
        return Err(HostLifetimeError::InvalidSelection);
    }
    let identity = host.console_setup_binding()?.with_consent_host(|owner| {
        ConsentApplicationIdentity::from_installed(owner)
            .map_err(|_| HostLifetimeError::Unavailable)
    })?;
    let owner = ApplicationConsentOwner::create(&control.directory, identity)
        .map_err(|_| HostLifetimeError::Unavailable)?;
    let human = PrincipalRef::in_domain(
        PrincipalKind::Human,
        "installed-console-human",
        TrustDomainId::new("oidc-issuer:https://installed-console-fixture.localhost")
            .map_err(|_| HostLifetimeError::InvalidSelection)?,
    )
    .map_err(|_| HostLifetimeError::InvalidSelection)?;
    let selection = ConsoleConsentSelection::new(
        owner.clone(),
        Arc::new(NoModelReview),
        Duration::from_secs(30),
        Duration::from_secs(600),
        control.origin.clone(),
        vec![ConsoleConsentRule::for_target(
            meerkat_mobkit::identity_first::AgentIdentity::parse("planning-lead")
                .map_err(|_| HostLifetimeError::InvalidSelection)?,
            vec![human],
        )?],
    )?;
    let mut slot = control
        .owner
        .lock()
        .map_err(|_| HostLifetimeError::Unavailable)?;
    if slot.is_some() {
        return Err(HostLifetimeError::InvalidSelection);
    }
    *slot = Some(owner);
    Ok(Some(selection))
}
pub(crate) fn observe_runtime(
    host: ToolkitGatewayHost,
) -> Result<ToolkitGatewayHost, HostLifetimeError> {
    match CONTROL.get() {
        Some(c) => host.with_runtime_observer_for_test(c.runtime.clone()),
        None => super::canonical_g_recovery::observe_runtime(host),
    }
}
pub(crate) fn observe_workflow(live: &Arc<LiveSharedSetup>) -> Result<(), HostLifetimeError> {
    if let Some(control) = CONTROL.get() {
        control
            .live
            .set(Arc::downgrade(live))
            .map_err(|_| HostLifetimeError::InvalidSelection)?;
    } else {
        super::canonical_g_recovery::observe_workflow(live)?;
    }
    Ok(())
}
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    approval_id: ApprovalId,
}
struct Server {
    stop: oneshot::Sender<()>,
    task: tokio::task::JoinHandle<Result<(), String>>,
}
impl Server {
    fn start(control: Arc<Control>) -> Result<Self, String> {
        // The isolated fixture TMPDIR can exceed sockaddr_un's pathname limit.
        // Keep this private socket owner in a fresh short directory, not ambient TMPDIR.
        let socket_root = if cfg!(target_os = "macos") {
            "/private/tmp"
        } else {
            "/tmp"
        };
        let directory = tempfile::Builder::new()
            .prefix("g-cap-")
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir_in(socket_root)
            .map_err(error)?;
        let socket = directory.path().join("owner.sock");
        let listener = UnixListener::bind(&socket).map_err(error)?;
        durable(
            &control.directory.join("socket.json"),
            &json!({"path":socket}),
        )?;
        let (stop, mut stopped) = oneshot::channel();
        let task = tokio::spawn(async move {
            let _directory = directory;
            let (mut stream, _) = tokio::select! {
                _ = &mut stopped => return Err("G capture was not completed".into()),
                accepted = listener.accept() => accepted.map_err(error)?,
            };
            let captured = async {
                let mut raw = Vec::new();
                (&mut stream)
                    .take(16_385)
                    .read_to_end(&mut raw)
                    .await
                    .map_err(error)?;
                check(raw.len() <= 16_384, "bounded capture command")?;
                let request: Request = serde_json::from_slice(&raw).map_err(error)?;
                let runtime = control
                    .runtime
                    .lock()
                    .map_err(error)?
                    .clone()
                    .ok_or("actual runtime absent")?;
                let service = runtime
                    .session_service()
                    .ok_or("actual erased service absent")?;
                let machine = service
                    .acquire_runtime_adapter(None)
                    .map_err(error)?
                    .ok_or("actual machine absent")?;
                let handle = runtime.handle();
                check(
                    !handle.is_public_owner(),
                    "observer must not own native shutdown",
                )?;
                let owner = control
                    .owner
                    .lock()
                    .map_err(error)?
                    .clone()
                    .ok_or("actual issuer absent")?;
                let record = owner.approvals().get(&request.approval_id).map_err(error)?;
                let scope = record
                    .consent
                    .as_ref()
                    .and_then(|c| c.scope.as_ref())
                    .ok_or("actual scoped G approval absent")?;
                check(
                    machine
                        .hosting_authority()
                        .capability()
                        .domain_ref()
                        .as_ref()
                        == Some(&scope.domain),
                    "approval domain differs from the exact captured native owner",
                )?;
                let session = &scope.session_id;
                let registration = machine
                    .current_session_registration_witness(session)
                    .await
                    .ok_or("actual G registration absent before hold")?;
                // Use the exact machine's public run-start gate, not MobHandle::stop,
                // cancellation or unregister. MobStop is the typed hold reason;
                // no mob lifecycle transition or consent withdrawal is requested.
                let held = machine
                    .hold_run_starts(session, meerkat_runtime::RunStartHoldReason::MobStop)
                    .await
                    .map_err(error)?;
                check(
                    held.current_run.is_none(),
                    "G still has a run at the actual hold boundary",
                )?;
                check(
                    owner.approvals().get(&request.approval_id).map_err(error)? == record,
                    "holding starts changed the full pending consent record",
                )?;
                let live = control
                    .live
                    .get()
                    .and_then(Weak::upgrade)
                    .ok_or("actual installed workflow absent")?;
                let mut capture = live
                    .capture_pending_canonical_g(&owner, &machine, &record)
                    .await?;
                let still_held = machine
                    .hold_run_starts(session, meerkat_runtime::RunStartHoldReason::MobStop)
                    .await
                    .map_err(error)?;
                check(
                    still_held.current_run.is_none()
                        && machine
                            .current_session_registration_witness(session)
                            .await
                            .as_ref()
                            == Some(&registration)
                        && !registration.has_ended()
                        && owner.approvals().get(&request.approval_id).map_err(error)? == record,
                    "same native registration/hold/approval must survive capture",
                )?;
                capture["native_run_start_hold"] = json!({"operation":"hold_run_starts","reason":"mob_stop",
                    "session_id":session,"current_run":held.current_run,"rechecked_current_run":still_held.current_run,
                    "same_registration":true,"mob_stop_invoked":false,"unregister_invoked":false,
                    "approval_before_hold":record});
                capture["pid"] = json!(std::process::id());
                capture["bootstrap"] = json!(control.bootstrap);
                capture["requested_exit_code"] = json!(EXIT);
                durable(&control.directory.join("g-capture.json"), &capture)?;
                Ok::<_, String>(capture)
            };
            let result = match tokio::time::timeout(Duration::from_secs(60), captured).await {
                Ok(Ok(value)) => json!({"status":"captured_predecessor","capture":value}),
                Ok(Err(error)) => json!({"status":"failed","error":error}),
                Err(error) => json!({"status":"failed","error":error.to_string()}),
            };
            durable(&control.directory.join("native-command.json"), &result)?;
            let raw = serde_json::to_vec(&result).map_err(error)?;
            check(raw.len() <= 8 * 1024 * 1024, "bounded capture response")?;
            tokio::time::timeout(Duration::from_secs(10), stream.write_all(&raw))
                .await
                .map_err(error)?
                .map_err(error)?;
            if result["status"] != "captured_predecessor" {
                return Err(result.to_string());
            }
            // Deliberate OS process loss, not ordinary Rust owner disposal.
            std::process::exit(EXIT);
        });
        Ok(Self { stop, task })
    }
    async fn finish(self) -> Result<(), String> {
        let _ = self.stop.send(());
        self.task.await.map_err(error)?
    }
}
fn child() -> ! {
    let fd: i32 = std::env::var("CONNECT_CANONICAL_G_STDOUT")
        .unwrap()
        .parse()
        .unwrap();
    assert!(fd >= 3);
    std::io::stdout().flush().unwrap();
    let protocol = fs::OpenOptions::new()
        .write(true)
        .open(format!("/dev/fd/{fd}"))
        .unwrap();
    nix::unistd::dup2_stdout(&protocol).unwrap();
    nix::unistd::close(fd).unwrap();
    drop(protocol);
    let args: Vec<String> = serde_json::from_str(&std::env::var(CHILD).unwrap()).unwrap();
    if let Some(code) = crate::application_bootstrap::run(&args) {
        std::process::exit(if code == std::process::ExitCode::SUCCESS {
            0
        } else {
            1
        });
    }
    let (mut read, mut write, mut persistent, mut workflows) = (None, None, false, false);
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--persistent" if !persistent => persistent = true,
            "--private-workflow-launch" if !workflows => workflows = true,
            "--host-control-read-fd" if read.is_none() => {
                read = Some(args.next().unwrap().parse::<i32>().unwrap())
            }
            "--host-control-write-fd" if write.is_none() => {
                write = Some(args.next().unwrap().parse::<i32>().unwrap())
            }
            _ => panic!("unexpected canonical G child argument"),
        }
    }
    assert!(persistent && workflows);
    let directory = PathBuf::from(std::env::var_os("CONNECT_CANONICAL_G_REPORT").unwrap());
    assert!(directory.is_absolute() && directory.is_dir());
    let runtime = meerkat_mobkit::gateway_composition::gateway_tokio_runtime().unwrap();
    let result = runtime.block_on(async {
        let pipes =
            toolkit_mobkit::private_host::InheritedHostPipes::adopt(read.unwrap(), write.unwrap())
                .map_err(error)?;
        let startup = PrivateHostStartup::read(pipes).await.map_err(error)?;
        match startup.intent() {
            PrivateHostStartupIntent::PrepareApplication(_) => {
                crate::application_driver::run_with_options(
                    startup,
                    None,
                    None,
                    crate::application_driver::ApplicationDriverOptions::default(),
                )
                .await
            }
            PrivateHostStartupIntent::Installed(bootstrap) => {
                let control = Arc::new(Control {
                    directory: directory.clone(),
                    origin: std::env::var("CONNECT_CANONICAL_G_ORIGIN").unwrap(),
                    owner: Mutex::new(None),
                    runtime: Arc::new(Mutex::new(None)),
                    live: OnceLock::new(),
                    bootstrap: bootstrap.clone(),
                });
                CONTROL
                    .set(control.clone())
                    .ok()
                    .expect("one installed G owner");
                let server = Server::start(control.clone())?;
                let result = run_with_options(
                    startup,
                    None,
                    None,
                    InstalledGatewayOptions {
                        private_workflows: true,
                        operation_review_policy: Some(control.clone()),
                        ..InstalledGatewayOptions::default()
                    },
                )
                .await;
                let server_result = server.finish().await;
                check(
                    control.runtime.lock().map_err(error)?.is_none(),
                    "native runtime observer survived close",
                )?;
                durable(
                    &directory.join("installed-return.json"),
                    &json!({"returned":true,"error":result.as_ref().err()}),
                )?;
                result.and(server_result)
            }
            _ => Err("G producer requires ordinary original-owner Restore".into()),
        }
    });
    let success = result.is_ok();
    runtime.shutdown_background();
    std::process::exit(if success { 0 } else { 1 });
}
#[test]
#[ignore = "requires exact G producer tuple, reviewed f25 writer, SDK/resources and fresh process capture"]
fn published_g_pending_consent_survives_actual_owner_loss() {
    if std::env::var_os(CHILD).is_some() {
        child();
    }
    let selected = |name| {
        let p = PathBuf::from(std::env::var_os(name).expect("explicit fixture input"));
        assert!(p.is_absolute());
        p
    };
    let output = selected("TOOLKIT_CANONICAL_G_CAPTURE_OUTPUT");
    assert!(!output.exists());
    let Some(resources) = option_env!("MEERKAT_CONNECT_RESOURCES_SHA256") else {
        panic!("resource-bound build");
    };
    assert_eq!(
        std::env::var("TOOLKIT_SHARED_PUBLIC_RESOURCES_SHA256").unwrap(),
        resources
    );
    let script = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../scripts/fixtures/r5_canonical_g_predecessor.py")
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
        "actual canonical G capture failed: {}",
        output.display()
    );
    let report: Value =
        serde_json::from_slice(&fs::read(output.join("result.json")).unwrap()).unwrap();
    assert_eq!(report["status"], "captured_predecessor");
    assert_eq!(report["cases"].as_array().unwrap().len(), 1);
    let path = output.join("g-owner/g-capture.json");
    let raw = fs::read(&path).unwrap();
    use sha2::Digest;
    assert_eq!(report["cases"][0]["capture"]["path"], json!(path));
    assert_eq!(report["cases"][0]["capture"]["bytes"], raw.len() as u64);
    assert_eq!(
        report["cases"][0]["capture"]["sha256"],
        format!("{:x}", sha2::Sha256::digest(&raw))
    );
    let capture: Value = serde_json::from_slice(&raw).unwrap();
    let identity = serde_json::from_value(capture["identity"].clone()).unwrap();
    let owner = ApplicationConsentOwner::reopen(
        serde_json::from_value(capture["issuer"].clone()).unwrap(),
        &identity,
    )
    .unwrap();
    let rows = owner
        .approvals()
        .list(ApprovalListFilter::default())
        .unwrap();
    assert_eq!(
        serde_json::to_value(&rows).unwrap(),
        capture["approvals"],
        "old process loss must preserve its pending row"
    );
    let association = serde_json::from_value(capture["association"].clone()).unwrap();
    let document = &capture["association_document"];
    let local = crate::approval::ClosedLocalApprovalsArtifact::read_for_association(
        &association,
        Path::new(document["approval_source"].as_str().unwrap()),
        &serde_json::from_value(capture["session_id"].clone()).unwrap(),
    )
    .unwrap();
    local.check().unwrap();
    assert_eq!(
        serde_json::to_value(local.artifact()).unwrap(),
        capture["presenter_closed"]
    );
    let physical: crate::resources::PhysicalSelection =
        serde_json::from_value(capture["physical"].clone()).unwrap();
    let token = physical
        .seal_no_entry()
        .expect("all exact old Controller/bridge owners must be excluded");
    let census = token.census();
    assert!(
        census["operations"].as_array().unwrap().is_empty(),
        "R3 must precede every physical dispatch"
    );
    let members = census["members"].as_array().unwrap();
    assert_eq!(members.len(), 1);
    assert_eq!(members[0]["session"], capture["session_id"]);
    assert_eq!(members[0]["member"]["member"], "planning-lead");
    token.recheck().unwrap();
    durable(&output.join("g-owner-loss-readback.json"),&json!({"scope":"same_G_issuer_presenter_and_exclusive_physical_no_entry",
        "association":association,"session_id":capture["session_id"],"issuer":owner.selection(),"identity":identity,
        "approvals":rows,"presenter_closed":local.artifact(),"physical":token.selection(),"physical_census":census,
        "native_input_capture_source":"LiveRuntime","native_R5_result":null,"fresh_recovery_preparation":null})).unwrap();
    // No claim/R5/replacement is run. A later reader must reacquire this exact
    // sealed physical lease and claim and revalidate the complete same-G history.
}
