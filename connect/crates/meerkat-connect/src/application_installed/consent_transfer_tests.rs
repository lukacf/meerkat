//! Explicit process control through the production installed gateway entry.
//! Meta52 lacks registered consent delivery: the transfer oracle must fail
//! until the same gateway delivery owner receives the upstream R6 composition.
use super::*;
use crate::application_consent::{ApplicationConsentOwner, ConsentApplicationIdentity};
use crate::application_gateway::console_consent::{ConsoleConsentRule, ConsoleConsentSelection};
use meerkat_core::{
    PrincipalKind, PrincipalRef, TrustDomainId,
    approval::{
        ApprovalListFilter,
        review::{OperationReviewer, ReviewCandidate, ReviewVerdict, ReviewerFailure},
    },
};
use meerkat_toolkit::host_lifetime::control::HostLifetimeError;
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::Command,
    sync::{Mutex, OnceLock},
    time::Duration,
};
use toolkit_mobkit::host_gateway::ToolkitGatewayHost;

const SELECTOR: &str = "application_installed::consent_transfer_tests::registered_console_consent_transfers_only_its_exact_notice_input";
mod cold_capture;
mod requeue;
const REQUEUE_SELECTOR: &str = "application_installed::consent_transfer_tests::parked_consent_requeues_through_the_same_installed_native_owner";
const CHILD: &str = "CONNECT_CONSENT_TRANSFER_CHILD";
static CONTROL: OnceLock<Arc<Control>> = OnceLock::new();
struct Control {
    directory: PathBuf,
    origin: String,
    owner: Mutex<Option<Arc<ApplicationConsentOwner>>>,
    requeue: bool,
    capture: bool,
    bootstrap: Mutex<Option<meerkat_toolkit::host_lifetime::control::HostBootstrapSelection>>,
    runtime: Arc<Mutex<Option<meerkat_mobkit::mob_handle_runtime::MobRuntime>>>,
}
pub(super) fn observe_runtime(
    host: ToolkitGatewayHost,
) -> Result<ToolkitGatewayHost, HostLifetimeError> {
    match CONTROL.get().filter(|control| control.requeue) {
        Some(control) => host.with_runtime_observer_for_test(control.runtime.clone()),
        None => super::canonical_g_capture::observe_runtime(host),
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
        let owner = self
            .owner
            .lock()
            .map_err(|_| meerkat_core::OperationAuthorizationError::Unavailable)?;
        let owner = owner
            .as_ref()
            .ok_or(meerkat_core::OperationAuthorizationError::Unavailable)?;
        crate::application_consent::fixture_review_floor::find(owner, context, &["original"])
    }
}
struct NoModelReview;
#[async_trait::async_trait]
impl OperationReviewer for NoModelReview {
    async fn review(&self, _: &ReviewCandidate<'_>) -> Result<ReviewVerdict, ReviewerFailure> {
        panic!("this control selects only the original's exact R3 Find");
    }
}
// Called only at the existing selection point, after opening the real installed
// owner and before Meta constructs any factory. Ordinary tests select nothing.
pub(super) fn select(
    host: &ToolkitGatewayHost,
    existing: Option<ConsoleConsentSelection>,
) -> Result<Option<ConsoleConsentSelection>, HostLifetimeError> {
    let Some(control) = CONTROL.get() else {
        return super::canonical_g_capture::select(host, existing);
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
            meerkat_mobkit::identity_first::AgentIdentity::parse("original")
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
fn save(path: &Path, value: &serde_json::Value) {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .unwrap();
    file.write_all(&serde_json::to_vec_pretty(value).unwrap())
        .unwrap();
    file.sync_all().unwrap();
}
fn child() -> ! {
    let fd: i32 = std::env::var("CONNECT_CONSENT_TRANSFER_STDOUT")
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
    let (mut read, mut write, mut persistent) = (None, None, false);
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--persistent" if !persistent => persistent = true,
            "--host-control-read-fd" if read.is_none() => {
                read = Some(args.next().unwrap().parse::<i32>().unwrap())
            }
            "--host-control-write-fd" if write.is_none() => {
                write = Some(args.next().unwrap().parse::<i32>().unwrap())
            }
            _ => panic!("unexpected consent child argument"),
        }
    }
    assert!(persistent);
    let directory = PathBuf::from(std::env::var_os("CONNECT_CONSENT_TRANSFER_REPORT").unwrap());
    assert!(directory.is_absolute() && directory.is_dir());
    let control = Arc::new(Control {
        directory: directory.clone(),
        origin: std::env::var("CONNECT_CONSENT_TRANSFER_ORIGIN").unwrap(),
        owner: Mutex::new(None),
        requeue: std::env::var_os("CONNECT_CONSENT_REQUEUE").is_some(),
        capture: std::env::var_os("CONNECT_CONSENT_CAPTURE").is_some(),
        bootstrap: Mutex::new(None),
        runtime: Arc::new(Mutex::new(None)),
    });
    let runtime = meerkat_mobkit::gateway_composition::gateway_tokio_runtime().unwrap();
    let result = runtime.block_on(async {
        let pipes =
            toolkit_mobkit::private_host::InheritedHostPipes::adopt(read.unwrap(), write.unwrap())
                .map_err(|error| error.to_string())?;
        let startup = PrivateHostStartup::read(pipes)
            .await
            .map_err(|error| error.to_string())?;
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
            PrivateHostStartupIntent::Installed(selection) => {
                if control.capture {
                    *control.bootstrap.lock().unwrap() = Some(selection.clone());
                }
                CONTROL
                    .set(control.clone())
                    .ok()
                    .expect("one actual installed selection");
                let server = if control.requeue {
                    Some(requeue::Server::start(control.clone())?)
                } else {
                    None
                };
                let result = run_with_options(
                    startup,
                    None,
                    None,
                    InstalledGatewayOptions {
                        operation_review_policy: Some(control.clone()),
                        ..InstalledGatewayOptions::default()
                    },
                )
                .await;
                let joined = match server {
                    Some(server) => server.finish().await,
                    None => Ok(()),
                };
                if control
                    .runtime
                    .lock()
                    .map_err(|_| "runtime observation poisoned")?
                    .is_some()
                {
                    return Err("native observer survived the host close boundary".into());
                }
                match (result, joined) {
                    (Ok(()), Ok(())) => Ok(()),
                    (Err(error), Ok(())) => Err(error),
                    (result, Err(error)) => {
                        Err(format!("{result:?}; native lifecycle control: {error}"))
                    }
                }
            }
            _ => Err("replacement startup is outside this control".into()),
        }
    });
    if CONTROL.get().is_some() {
        let owner = control.owner.lock().unwrap().take();
        let rows = owner.as_ref().map(|owner| {
            owner
                .approvals()
                .list(ApprovalListFilter::default())
                .map_err(|error| error.to_string())
        });
        save(
            &directory.join("installed-return.json"),
            &serde_json::json!({
                "runtime_observation_released":control.runtime.lock().unwrap().is_none(),
                "returned_ok":result.is_ok(),"error":result.as_ref().err(),"approvals":rows,
                "issuer":owner.as_ref().map(|owner|owner.selection()),
                "identity":owner.as_ref().map(|owner|owner.identity()),
            }),
        );
        drop(owner);
    }
    let successful = result.is_ok();
    runtime.shutdown_background();
    std::process::exit(if successful { 0 } else { 1 });
}

#[test]
#[ignore = "requires selected SDK/resources and a fresh retained process-control output"]
fn registered_console_consent_transfers_only_its_exact_notice_input() {
    run_selected(SELECTOR, false);
}

#[test]
#[ignore = "requires selected SDK/resources and a fresh retained process-control output"]
fn parked_consent_requeues_through_the_same_installed_native_owner() {
    run_selected(REQUEUE_SELECTOR, true);
}

fn run_selected(selector: &str, requeue: bool) {
    if std::env::var_os(CHILD).is_some() {
        child();
    }
    let selected = |name| {
        let path = PathBuf::from(std::env::var_os(name).expect("explicit fixture input"));
        assert!(path.is_absolute());
        path
    };
    let output = selected(if requeue {
        "TOOLKIT_CONSENT_REQUEUE_OUTPUT"
    } else {
        "TOOLKIT_CONSENT_TRANSFER_OUTPUT"
    });
    assert!(!output.exists());
    let Some(resources) = option_env!("MEERKAT_CONNECT_RESOURCES_SHA256") else {
        panic!("resource-bound test build");
    };
    assert_eq!(
        std::env::var("TOOLKIT_SHARED_PUBLIC_RESOURCES_SHA256").unwrap(),
        resources
    );
    let script = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../scripts/fixtures/registered_consent_transfer.py")
        .canonicalize()
        .unwrap();
    let mut command = Command::new(selected("TOOLKIT_SHARED_TEST_PYTHON"));
    command
        .args(["-I", "-B"])
        .arg(script)
        .arg("--test-executable")
        .arg(std::env::current_exe().unwrap())
        .arg("--selector")
        .arg(selector)
        .arg("--resource-executable")
        .arg(selected("TOOLKIT_SHARED_PUBLIC_EXECUTABLE"))
        .arg("--resources-sha256")
        .arg(resources)
        .arg("--toolkit-python")
        .arg(selected("TOOLKIT_LATE_ADOPTION_TOOLKIT_PYTHON"))
        .arg("--meta-python")
        .arg(selected("TOOLKIT_LATE_ADOPTION_META_PYTHON"))
        .arg("--output")
        .arg(&output);
    if requeue {
        command.arg("--requeue");
    }
    let status = command.status().unwrap();
    assert!(
        status.success(),
        "registered consent transfer failed: {}",
        output.display()
    );
    let result: serde_json::Value =
        serde_json::from_slice(&fs::read(output.join("result.json")).unwrap()).unwrap();
    assert_eq!(result["status"], "passed");
    assert_eq!(result["cases"].as_array().unwrap().len(), 2);
    for case in result["cases"].as_array().unwrap() {
        assert_eq!(case["status"], "passed");
        assert_eq!(case["registered_notice_transfer"], true);
        if requeue {
            assert_eq!(case["same_owner_requeue"], true);
        }
    }
}
