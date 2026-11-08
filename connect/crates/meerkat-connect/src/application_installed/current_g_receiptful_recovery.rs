//! Separate current-behavior reader of a genuinely captured receiptful recovered boundary.
//! Same source and artifacts supply both phases. Historical207 remains unchanged and required.
use super::*;
use crate::{
    application_consent::{ApplicationConsentOwner, ConsentApplicationIdentity, ConsentOwnerError},
    application_gateway::console_consent::{ConsoleConsentRule, ConsoleConsentSelection},
    installed_service_review::composer::LiveSharedSetup,
};
use meerkat_core::{
    PrincipalKind, PrincipalRef, TrustDomainId,
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
const SELECTOR: &str = "application_installed::current_g_receiptful_recovery::same_g_receiptful_boundary_reopens_and_accepts_fresh_work";
const CHILD: &str = "CONNECT_CURRENT_RECEIPTFUL_G_READER_CHILD";
static CONTROL: OnceLock<Arc<Control>> = OnceLock::new();
struct Control {
    directory: PathBuf,
    capture: Value,
    recovered: Value,
    origin: String,
    owner: Mutex<Option<Arc<ApplicationConsentOwner>>>,
    live: OnceLock<Weak<LiveSharedSetup>>,
    runtime: Arc<Mutex<Option<meerkat_mobkit::mob_handle_runtime::MobRuntime>>>,
}
fn error(e: impl std::fmt::Display) -> String {
    e.to_string()
}
fn check(ok: bool, why: &str) -> Result<(), String> {
    if ok { Ok(()) } else { Err(why.into()) }
}
fn durable(path: &Path, value: &Value) -> Result<(), String> {
    let mut f = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(error)?;
    f.write_all(&serde_json::to_vec_pretty(value).map_err(error)?)
        .map_err(error)?;
    f.sync_all().map_err(error)?;
    fs::File::open(path.parent().ok_or("no output parent")?)
        .and_then(|f| f.sync_all())
        .map_err(error)
}
pub(crate) fn predecessor() -> Option<Value> {
    CONTROL.get().map(|c| c.capture.clone())
}
pub(crate) fn recovered() -> Option<Value> {
    CONTROL.get().map(|c| c.recovered.clone())
}
pub(crate) fn record(name: &str, value: &Value) -> Result<(), HostLifetimeError> {
    let Some(c) = CONTROL.get() else {
        return Ok(());
    };
    if !matches!(name, "before-native" | "native-repeat" | "one-shot") {
        return Err(HostLifetimeError::InvalidSelection);
    }
    durable(&c.directory.join(format!("{name}.json")), value)
        .map_err(|_| HostLifetimeError::Unavailable)
}
struct NoReview;
#[async_trait::async_trait]
impl OperationReviewer for NoReview {
    async fn review(&self, _: &ReviewCandidate<'_>) -> Result<ReviewVerdict, ReviewerFailure> {
        panic!("same-G read control must not ask an R2 reviewer")
    }
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
        if !matches!(context.target(),meerkat_core::ToolOperationTarget::McpServerOperation{raw_operation,..}
            if raw_operation=="setup_state")
        {
            return Ok(OperationReviewTier::R1);
        }
        let owner = self
            .owner
            .lock()
            .map_err(|_| OperationAuthorizationError::Unavailable)?
            .clone()
            .ok_or(OperationAuthorizationError::Unavailable)?;
        self.live
            .get()
            .and_then(Weak::upgrade)
            .ok_or(OperationAuthorizationError::Unavailable)?
            .canonical_g_review_floor(&owner, context)
    }
}
pub(crate) fn select(
    host: &ToolkitGatewayHost,
    existing: Option<ConsoleConsentSelection>,
) -> Result<Option<ConsoleConsentSelection>, HostLifetimeError> {
    let Some(c) = CONTROL.get() else {
        return super::consent_transfer_tests::select(host, existing);
    };
    if existing.is_some() {
        return Err(HostLifetimeError::InvalidSelection);
    }
    let identity = host.console_setup_binding()?.with_consent_host(|owner| {
        ConsentApplicationIdentity::from_installed(owner)
            .map_err(|_| HostLifetimeError::Unavailable)
    })?;
    if json!(identity) != c.capture["identity"] {
        return Err(HostLifetimeError::WrongOwner);
    }
    let selected = serde_json::from_value(c.capture["issuer"].clone())
        .map_err(|_| HostLifetimeError::InvalidSelection)?;
    let owner = match ApplicationConsentOwner::reopen(selected, &identity) {
        Ok(owner) => owner,
        Err(ConsentOwnerError::OwnerHeld) => {
            durable(&c.directory.join("issuer-refusal.json"),&json!({"category":"owner_held",
                "phase":"before_runtime_observer","identity":identity,"issuer":c.capture["issuer"]}))
                .map_err(|_|HostLifetimeError::Unavailable)?;
            return Err(HostLifetimeError::Unavailable);
        }
        Err(_) => return Err(HostLifetimeError::Unavailable),
    };
    let human = PrincipalRef::in_domain(
        PrincipalKind::Human,
        "installed-console-human",
        TrustDomainId::new("oidc-issuer:https://installed-console-fixture.localhost")
            .map_err(|_| HostLifetimeError::InvalidSelection)?,
    )
    .map_err(|_| HostLifetimeError::InvalidSelection)?;
    let selection = ConsoleConsentSelection::new(
        owner.clone(),
        Arc::new(NoReview),
        Duration::from_secs(30),
        Duration::from_secs(600),
        c.origin.clone(),
        vec![ConsoleConsentRule::for_target(
            meerkat_mobkit::identity_first::AgentIdentity::parse("planning-lead")
                .map_err(|_| HostLifetimeError::InvalidSelection)?,
            vec![human],
        )?],
    )?;
    let mut slot = c.owner.lock().map_err(|_| HostLifetimeError::Unavailable)?;
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
        None => super::consent_transfer_tests::observe_runtime(host),
    }
}
pub(crate) fn observe_workflow(live: &Arc<LiveSharedSetup>) -> Result<(), HostLifetimeError> {
    if let Some(c) = CONTROL.get() {
        c.live
            .set(Arc::downgrade(live))
            .map_err(|_| HostLifetimeError::InvalidSelection)?;
    } else {
        super::canonical_g_capture::observe_workflow(live)?;
    }
    Ok(())
}
struct Server {
    stop: oneshot::Sender<()>,
    task: tokio::task::JoinHandle<Result<(), String>>,
}
impl Server {
    fn start(c: Arc<Control>) -> Result<Self, String> {
        // The isolated fixture TMPDIR can exceed sockaddr_un's pathname limit.
        // Keep this private socket owner in a fresh short directory, not ambient TMPDIR.
        let socket_root = if cfg!(target_os = "macos") {
            "/private/tmp"
        } else {
            "/tmp"
        };
        let directory = tempfile::Builder::new()
            .prefix("g-read-")
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir_in(socket_root)
            .map_err(error)?;
        let path = directory.path().join("owner.sock");
        let listener = UnixListener::bind(&path).map_err(error)?;
        durable(&c.directory.join("socket.json"), &json!({"path":path}))?;
        let (stop, mut stopped) = oneshot::channel();
        let task = tokio::spawn(async move {
            let _directory = directory;
            let mut phases = Vec::new();
            loop {
                let (mut stream, _) = tokio::select! {_=&mut stopped=>return Ok(()),r=listener.accept()=>r.map_err(error)?};
                let operation = async {
                    let mut raw = Vec::new();
                    (&mut stream)
                        .take(1025)
                        .read_to_end(&mut raw)
                        .await
                        .map_err(error)?;
                    check(raw.len() <= 1024, "bounded observation request")?;
                    let request: Value = serde_json::from_slice(&raw).map_err(error)?;
                    let phase = request
                        .get("phase")
                        .and_then(Value::as_str)
                        .ok_or("missing phase")?;
                    check(
                        request.as_object().is_some_and(|r| r.len() == 1)
                            && phases.len() < 2
                            && phase
                                == if phases.is_empty() {
                                    "ready"
                                } else {
                                    "after-work"
                                },
                        "ordered observation phases",
                    )?;
                    let live = c
                        .live
                        .get()
                        .and_then(Weak::upgrade)
                        .ok_or("actual G owner absent")?;
                    let value = live.inspect_recovered_current_g().await.map_err(error)?;
                    let runtime = c
                        .runtime
                        .lock()
                        .map_err(error)?
                        .clone()
                        .ok_or("actual Meta runtime absent")?;
                    let service = runtime
                        .session_service()
                        .ok_or("actual erased service absent")?;
                    let machine = service
                        .acquire_runtime_adapter(None)
                        .map_err(error)?
                        .ok_or("machine absent")?;
                    let session =
                        serde_json::from_value(c.capture["session_id"].clone()).map_err(error)?;
                    check(
                        machine
                            .current_session_registration_witness(&session)
                            .await
                            .is_some(),
                        "actual G registration absent",
                    )?;
                    durable(&c.directory.join(format!("{phase}.json")), &value)?;
                    phases.push(phase.to_owned());
                    Ok::<_, String>(json!({"status":"observed","phase":phase,"value":value}))
                };
                let result = match tokio::time::timeout(Duration::from_secs(60), operation).await {
                    Ok(Ok(v)) => v,
                    Ok(Err(e)) => json!({"status":"failed","error":e}),
                    Err(e) => json!({"status":"failed","error":e.to_string()}),
                };
                let raw = serde_json::to_vec(&result).map_err(error)?;
                check(raw.len() <= 8 * 1024 * 1024, "bounded native observation")?;
                tokio::time::timeout(Duration::from_secs(10), stream.write_all(&raw))
                    .await
                    .map_err(error)?
                    .map_err(error)?;
                if result["status"] != "observed" {
                    return Err(result.to_string());
                }
            }
        });
        Ok(Self { stop, task })
    }
    async fn finish(self) -> Result<(), String> {
        let _ = self.stop.send(());
        self.task.await.map_err(error)?
    }
}
fn child() -> ! {
    let fd: i32 = std::env::var("CONNECT_CURRENT_RECEIPTFUL_G_READER_STDOUT")
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
    // This consumer never handles PrepareApplication or emits another plan.
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
            _ => panic!("unexpected same-G consumer argument"),
        }
    }
    assert!(persistent && workflows);
    let directory =
        PathBuf::from(std::env::var_os("CONNECT_CURRENT_RECEIPTFUL_G_READER_REPORT").unwrap());
    let capture_path =
        PathBuf::from(std::env::var_os("CONNECT_CURRENT_RECEIPTFUL_G_READER_CAPTURE").unwrap());
    assert!(directory.is_absolute() && directory.is_dir() && capture_path.is_absolute());
    let capture: Value = serde_json::from_slice(&fs::read(&capture_path).unwrap()).unwrap();
    let recovered: Value = serde_json::from_slice(
        &fs::read(
            capture_path
                .parent()
                .unwrap()
                .parent()
                .unwrap()
                .join("RECOVERED.json"),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(
        recovered["status"],
        "same_G_current_boundary_receipt_pending"
    );
    assert_eq!(recovered["session"], capture["session_id"]);
    assert_eq!(recovered["input"], capture["origin_input_id"]);
    assert_eq!(recovered["run"], capture["origin_run_id"]);
    let runtime = meerkat_mobkit::gateway_composition::gateway_tokio_runtime().unwrap();
    let result = runtime.block_on(async {
        let pipes =
            toolkit_mobkit::private_host::InheritedHostPipes::adopt(read.unwrap(), write.unwrap())
                .map_err(error)?;
        let startup = PrivateHostStartup::read(pipes).await.map_err(error)?;
        check(
            matches!(startup.intent(), PrivateHostStartupIntent::Installed(_)),
            "same-G requires exact installed Restore",
        )?;
        let selection = crate::application_consent::AbandonedWorkflowRecovery::new(
            vec![serde_json::from_value(capture["association"].clone()).map_err(error)?],
            Duration::from_secs(180),
        )
        .map_err(error)?;
        let c = Arc::new(Control {
            directory: directory.clone(),
            capture,
            recovered,
            origin: std::env::var("CONNECT_CURRENT_RECEIPTFUL_G_READER_ORIGIN").unwrap(),
            owner: Mutex::new(None),
            runtime: Arc::new(Mutex::new(None)),
            live: OnceLock::new(),
        });
        CONTROL
            .set(c.clone())
            .ok()
            .expect("one actual same-G consumer");
        let server = Server::start(c.clone())?;
        let result = run_with_options(
            startup,
            None,
            None,
            InstalledGatewayOptions {
                private_workflows: true,
                operation_review_policy: Some(c.clone()),
                abandoned_workflows: Some(selection),
                ..InstalledGatewayOptions::default()
            },
        )
        .await;
        let observed = server.finish().await;
        check(
            c.runtime.lock().map_err(error)?.is_none(),
            "runtime observer survived native close",
        )?;
        durable(
            &directory.join("installed-return.json"),
            &json!({"returned":true,"runtime_observation_released":true,
            "error":result.as_ref().err(),"observer_error":observed.as_ref().err()}),
        )?;
        result.and(observed)
    });
    if let Err(failure) = &result {
        // Diagnostics must not replace the original child failure or change its exit.
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = durable(
                &directory.join("child-failure.json"),
                &json!({"status":"failed","error":failure}),
            );
            let _ = writeln!(
                std::io::stderr(),
                "current receiptful G reader child failed: {failure}"
            );
        }));
    }
    let success = result.is_ok();
    runtime.shutdown_background();
    std::process::exit(if success { 0 } else { 1 })
}
#[test]
#[ignore = "requires genuine current same-G receiptful capture and exact same-tuple artifact proof"]
fn same_g_receiptful_boundary_reopens_and_accepts_fresh_work() {
    if std::env::var_os(CHILD).is_some() {
        child();
    }
    let path = |key| {
        let p = PathBuf::from(std::env::var_os(key).expect("explicit selected input"));
        assert!(p.is_absolute());
        p
    };
    let predecessor = path("TOOLKIT_CURRENT_RECEIPTFUL_G_READER_PREDECESSOR");
    let output = path("TOOLKIT_CURRENT_RECEIPTFUL_G_READER_OUTPUT");
    assert!(predecessor.is_dir() && !output.exists());
    fs::create_dir(&output).unwrap();
    let capture: Value =
        serde_json::from_slice(&fs::read(predecessor.join("g-owner/g-capture.json")).unwrap())
            .unwrap();
    let identity = serde_json::from_value(capture["identity"].clone()).unwrap();
    let held = ApplicationConsentOwner::reopen(
        serde_json::from_value(capture["issuer"].clone()).unwrap(),
        &identity,
    )
    .unwrap();
    let before = held
        .approvals()
        .list(ApprovalListFilter::default())
        .unwrap();
    assert_eq!(json!(before), capture["approvals"]);
    let script = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../scripts/fixtures/current_g_receiptful_recovery.py")
        .canonicalize()
        .unwrap();
    let Some(resources) = option_env!("MEERKAT_CONNECT_RESOURCES_SHA256") else {
        panic!("exact resource-bound build");
    };
    assert_eq!(
        std::env::var("TOOLKIT_SHARED_PUBLIC_RESOURCES_SHA256").unwrap(),
        resources
    );
    let run = |name: &str, negative: bool| {
        let mut cmd = Command::new(path("TOOLKIT_SHARED_TEST_PYTHON"));
        cmd.args(["-I", "-B"])
            .arg(&script)
            .arg("--test-executable")
            .arg(std::env::current_exe().unwrap())
            .arg("--selector")
            .arg(SELECTOR)
            .arg("--resource-executable")
            .arg(path("TOOLKIT_SHARED_PUBLIC_EXECUTABLE"))
            .arg("--resources-sha256")
            .arg(resources)
            .arg("--toolkit-python")
            .arg(path("TOOLKIT_LATE_ADOPTION_TOOLKIT_PYTHON"))
            .arg("--meta-python")
            .arg(path("TOOLKIT_LATE_ADOPTION_META_PYTHON"))
            .arg("--predecessor")
            .arg(&predecessor)
            .arg("--bindings")
            .arg(path("TOOLKIT_CURRENT_RECEIPTFUL_G_READER_BINDINGS"))
            .arg("--binding-review")
            .arg(path("TOOLKIT_CURRENT_RECEIPTFUL_G_READER_BINDING_REVIEW"))
            .arg("--output")
            .arg(output.join(name));
        if negative {
            cmd.arg("--expect-owner-held");
        }
        let status = cmd.status().unwrap();
        assert!(
            status.success(),
            "same-G {name} failed: {}",
            output.display()
        );
    };
    run("live-issuer-refusal", true);
    assert_eq!(
        held.approvals()
            .list(ApprovalListFilter::default())
            .unwrap(),
        before
    );
    durable(
        &output.join("live-issuer-refusal-readback.json"),
        &json!({"same_issuer":held.selection(),
        "association":capture["association"],"session":capture["session_id"],"before":before,
        "after":held.approvals().list(ApprovalListFilter::default()).unwrap(),"changed":false}),
    )
    .unwrap();
    drop(held);
    run("same-g-restored", false);
    let result: Value =
        serde_json::from_slice(&fs::read(output.join("same-g-restored/result.json")).unwrap())
            .unwrap();
    assert_eq!(result["status"], "passed");
    assert_eq!(
        result["scope"],
        "current_same_g_receiptful_recovery_and_fresh_read"
    );
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
        json!(rows),
        result["after"]["issuer_rows"],
        "joined reader changed durable consent history"
    );
    use sha2::Digest;
    let pin = |p: PathBuf| {
        let raw = fs::read(&p).unwrap();
        json!({"path":p,"sha256":format!("{:x}",sha2::Sha256::digest(&raw)),"bytes":raw.len()})
    };
    durable(&output.join("result.json"),&json!({"status":"passed","scope":"current_same_g_receiptful_recovery_and_fresh_read",
        "predecessor":pin(predecessor.join("result.json")),"association":capture["association"],"session":capture["session_id"],
        "issuer_refusal":pin(output.join("live-issuer-refusal/result.json")),
        "issuer_refusal_readback":pin(output.join("live-issuer-refusal-readback.json")),
        "recovery":pin(output.join("same-g-restored/result.json")),"joined_issuer_rows":rows,
        "unrelated_nondirected_or_original_a_evidence_used":false,
        "current_native_producer":pin(predecessor.join("PRODUCER-COMPLETE.json")),
        "artifact_binding":pin(path("TOOLKIT_CURRENT_RECEIPTFUL_G_READER_BINDINGS")),
        "artifact_binding_review":pin(path("TOOLKIT_CURRENT_RECEIPTFUL_G_READER_BINDING_REVIEW"))})).unwrap();
}
