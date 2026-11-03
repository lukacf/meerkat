//! Opt-in process acceptance through the real Driver and Meta Ready path.
//! Late adoption forces the defensive ordering before selection. The separate
//! drain control only observes Meta's actual successful shutdown stop callback.
//! Neither control qualifies SIGTERM or a shipped product binary.
use super::*;
use std::{fs, io::Write, process::Command, sync::Weak};
use toolkit_mobkit::installed_host::acquisition::{
    AcquisitionRecoveryObservation, AcquisitionRecoveryTaskObservation as Task,
};

const SELECTOR: &str = "application_driver::late_adoption_tests::late_adoption_fences_actual_acquisition_before_native_ready_and_joins";
const CHILD: &str = "CONNECT_LATE_ADOPTION_TEST_CHILD";

pub(super) struct Probe {
    fenced: bool,
    drain: bool,
    directory: PathBuf,
    retained: OnceLock<Weak<ToolkitGatewayHost>>,
}
impl Probe {
    pub(super) fn stopped(&self, driver: &Driver) -> Result<(), GatewayHostError> {
        if !self.drain {
            return Ok(());
        }
        // Only the actual successful Meta callback reaches this observation.
        // The stop method released its publication guard before calling us.
        let host = driver.installed()?;
        let same_owner = self
            .retained
            .get()
            .and_then(Weak::upgrade)
            .is_some_and(|retained| Arc::ptr_eq(&retained, &host));
        let state = driver
            .publication
            .lock()
            .map_err(|_| GatewayHostError::Unavailable)?;
        let observation = host.observe_acquisition_recovery()?;
        if !same_owner
            || state.closed
            || !state.admission_stopped
            || !observation.stopped
            || !observation.selected_owner_control
            || !matches!(&observation.task, Task::Running | Task::Returned)
            || self.directory.join("stop-admitting.json").exists()
        {
            return Err(GatewayHostError::Denied);
        }
        let value = serde_json::json!({
            "same_installed_owner": same_owner, "closed": state.closed,
            "admission_stopped": state.admission_stopped, "recovery": observation,
        });
        drop(state);
        self.save("stop-admitting-writing.json", value)?;
        // A reader sees complete bytes, never an in-progress observation.
        fs::rename(
            self.directory.join("stop-admitting-writing.json"),
            self.directory.join("stop-admitting.json"),
        )
        .map_err(|_| GatewayHostError::Unavailable)
    }
    fn save(&self, name: &str, value: serde_json::Value) -> Result<(), GatewayHostError> {
        let bytes = serde_json::to_vec(&value).map_err(|_| GatewayHostError::Unavailable)?;
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(self.directory.join(name))
            .and_then(|mut file| file.write_all(&bytes))
            .map_err(|_| GatewayHostError::Unavailable)
    }
    pub(super) fn before_selection(&self, driver: &Driver) -> Result<(), GatewayHostError> {
        {
            let state = driver
                .publication
                .lock()
                .map_err(|_| GatewayHostError::Unavailable)?;
            if state.closed
                || state.installed.is_some()
                || state.admission_stopped
                || driver.completed.get().is_none()
                || driver.plan.acquisition.is_none()
            {
                return Err(GatewayHostError::Denied);
            }
        }
        if self.fenced {
            driver.stop_admitting(GatewayHostCloseReason::Shutdown)?;
        }
        let state = driver
            .publication
            .lock()
            .map_err(|_| GatewayHostError::Unavailable)?;
        self.save(
            "before-selection.json",
            serde_json::json!({
                "native_held_completion": true, "installed": state.installed.is_some(),
                "closed": state.closed, "admission_stopped": state.admission_stopped,
            }),
        )
    }
    pub(super) fn retained(
        &self,
        host: &Arc<ToolkitGatewayHost>,
        stopped: bool,
    ) -> Result<(), GatewayHostError> {
        if stopped != self.fenced || self.retained.set(Arc::downgrade(host)).is_err() {
            return Err(GatewayHostError::Denied);
        }
        self.observe("retained", host)
    }
    pub(super) fn observe(
        &self,
        stage: &str,
        host: &Arc<ToolkitGatewayHost>,
    ) -> Result<(), GatewayHostError> {
        let same_owner = self
            .retained
            .get()
            .and_then(Weak::upgrade)
            .is_some_and(|retained| Arc::ptr_eq(&retained, host));
        let observation = host.observe_acquisition_recovery()?;
        let expected = AcquisitionRecoveryObservation {
            stopped: self.fenced,
            selected_owner_control: stage == "after_ready",
            task: if stage == "after_ready" && !self.fenced {
                Task::Running
            } else {
                Task::Absent
            },
        };
        self.save(
            &format!("{stage}.json"),
            serde_json::json!({
                "same_installed_owner": same_owner, "recovery": &observation,
            }),
        )?;
        if !same_owner || observation != expected {
            return Err(GatewayHostError::Denied);
        }
        Ok(())
    }
    pub(super) fn joined(&self, driver: &Driver, all_joined: bool) -> Result<(), GatewayHostError> {
        let host = driver.installed()?;
        if !self
            .retained
            .get()
            .and_then(Weak::upgrade)
            .is_some_and(|retained| Arc::ptr_eq(&retained, &host))
        {
            return Err(GatewayHostError::Denied);
        }
        let observed = host.observe_acquisition_recovery()?;
        if !all_joined
            || observed
                != (AcquisitionRecoveryObservation {
                    stopped: true,
                    selected_owner_control: true,
                    task: if self.fenced {
                        Task::Absent
                    } else {
                        Task::Finished
                    },
                })
        {
            return Err(GatewayHostError::Denied);
        }
        // Meta owns the joined close. A replay must observe the same result.
        let path = self.directory.join("joined.json");
        let value = serde_json::json!({"same_installed_owner": true,
            "host_source_local_shared_joined": all_joined, "recovery": observed});
        if path.exists() {
            if serde_json::from_slice::<serde_json::Value>(
                &fs::read(path).map_err(|_| GatewayHostError::Unavailable)?,
            )
            .ok()
            .as_ref()
                != Some(&value)
            {
                return Err(GatewayHostError::Denied);
            }
            Ok(())
        } else {
            self.save("joined.json", value)
        }
    }
}

fn child(drain: bool) -> ! {
    // The test-only launcher starts libtest with stdout on /dev/null. Restore
    // the inherited protocol pipe only inside this selected test, before any
    // artifact/gateway producer or runtime is entered. No harness text reaches
    // either JSON protocol, and the wrapper exec preserves the native PID.
    let stdout_fd: i32 = std::env::var("CONNECT_LATE_ADOPTION_TEST_STDOUT")
        .unwrap()
        .parse()
        .unwrap();
    assert!(stdout_fd >= 3);
    std::io::stdout().flush().unwrap();
    let stdout = fs::OpenOptions::new()
        .write(true)
        .open(format!("/dev/fd/{stdout_fd}"))
        .unwrap();
    nix::unistd::dup2_stdout(&stdout).unwrap();
    nix::unistd::close(stdout_fd).unwrap();
    drop(stdout);
    let args: Vec<String> = serde_json::from_str(&std::env::var(CHILD).unwrap()).unwrap();
    // The existing finalizer invokes the actual native artifact commands in
    // separate processes. No replacement manifest/authority producer is used.
    if let Some(code) = crate::application_bootstrap::run(&args) {
        std::process::exit(if code == std::process::ExitCode::SUCCESS {
            0
        } else {
            1
        });
    }
    let mut read = None;
    let mut write = None;
    let mut persistent = false;
    let mut values = args.iter();
    while let Some(arg) = values.next() {
        match arg.as_str() {
            "--persistent" if !persistent => persistent = true,
            "--host-control-read-fd" if read.is_none() => {
                read = Some(values.next().unwrap().parse::<i32>().unwrap())
            }
            "--host-control-write-fd" if write.is_none() => {
                write = Some(values.next().unwrap().parse::<i32>().unwrap())
            }
            _ => panic!("unexpected late-adoption child argument"),
        }
    }
    assert!(persistent);
    let pipes =
        toolkit_mobkit::private_host::InheritedHostPipes::adopt(read.unwrap(), write.unwrap())
            .unwrap();
    let directory = PathBuf::from(std::env::var_os("CONNECT_LATE_ADOPTION_TEST_REPORT").unwrap());
    assert!(directory.is_absolute() && directory.is_dir());
    let fenced = match (
        drain,
        std::env::var("CONNECT_LATE_ADOPTION_TEST_FENCE")
            .unwrap()
            .as_str(),
    ) {
        (false, "fenced") => true,
        (false, "unfenced") | (true, "drain") => false,
        _ => panic!("unexpected test selection"),
    };
    let probe = Arc::new(Probe {
        fenced,
        drain,
        directory: directory.clone(),
        retained: OnceLock::new(),
    });
    if drain {
        // Expected typed value only, not a fabricated runtime observation.
        probe
            .save(
                "expected-refusal.json",
                meerkat_core::ToolError::unavailable(
                    toolkit_mobkit::FIND_AND_INSTALL,
                    meerkat_core::ToolUnavailableReason::TemporarilyUnavailable,
                )
                .to_error_payload(),
            )
            .unwrap();
    }
    let runtime = meerkat_mobkit::gateway_composition::gateway_tokio_runtime().unwrap();
    let result = runtime.block_on(async {
        let startup = PrivateHostStartup::read(pipes)
            .await
            .map_err(|error| error.to_string())?;
        run_with_options(
            startup,
            None,
            None,
            ApplicationDriverOptions {
                late_adoption: Some(probe),
                ..Default::default()
            },
        )
        .await
    });
    // run_with_options already joined native, source and private ownership.
    // Tokio's blocking stdin helper is not an unfinished native effect.
    let successful = result.is_ok();
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(directory.join("driver-return.json"))
        .unwrap();
    file.write_all(
        &serde_json::to_vec(&serde_json::json!({
            "returned_ok": successful, "error": result.err(),
        }))
        .unwrap(),
    )
    .unwrap();
    runtime.shutdown_background();
    std::process::exit(if successful { 0 } else { 1 });
}

#[test]
#[ignore = "requires explicitly selected native resources, SDK roots and retained output"]
fn late_adoption_fences_actual_acquisition_before_native_ready_and_joins() {
    run_acceptance(
        false,
        "late_driver_adoption.py",
        SELECTOR,
        "TOOLKIT_LATE_ADOPTION_OUTPUT",
        2,
    );
}

#[test]
#[ignore = "requires explicitly selected native resources, SDK roots and retained output"]
fn nonzero_drain_refuses_new_helper_and_completes_entered_console_turn() {
    run_acceptance(
        true,
        "nonzero_driver_drain.py",
        "application_driver::late_adoption_tests::nonzero_drain_refuses_new_helper_and_completes_entered_console_turn",
        "TOOLKIT_NONZERO_DRAIN_OUTPUT",
        1,
    );
}

fn run_acceptance(drain: bool, script: &str, selector: &str, output_key: &str, count: usize) {
    if std::env::var_os(CHILD).is_some() {
        child(drain);
    }
    let selected = |name| {
        let path =
            PathBuf::from(std::env::var_os(name).expect("explicit retained acceptance input"));
        assert!(path.is_absolute());
        path
    };
    let python = selected("TOOLKIT_SHARED_TEST_PYTHON");
    let output = selected(output_key);
    assert!(!output.exists(), "acceptance evidence is never overwritten");
    let Some(embedded) = option_env!("MEERKAT_CONNECT_RESOURCES_SHA256") else {
        panic!("resource-bound test build");
    };
    assert_eq!(
        std::env::var("TOOLKIT_SHARED_PUBLIC_RESOURCES_SHA256").unwrap(),
        embedded
    );
    let script = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../scripts/fixtures")
        .join(script)
        .canonicalize()
        .unwrap();
    let status = Command::new(python)
        .args(["-I", "-B"])
        .arg(script)
        .arg("--test-executable")
        .arg(std::env::current_exe().unwrap())
        .arg("--selector")
        .arg(selector)
        .arg("--resource-executable")
        .arg(selected("TOOLKIT_SHARED_PUBLIC_EXECUTABLE"))
        .arg("--resources-sha256")
        .arg(embedded)
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
        "real Driver acceptance failed; retained output: {}",
        output.display()
    );
    let result: serde_json::Value =
        serde_json::from_slice(&fs::read(output.join("result.json")).unwrap()).unwrap();
    assert_eq!(result["status"], "passed");
    assert_eq!(result["cases"].as_array().unwrap().len(), count);
    if drain {
        // Exact native completion is mandatory in either case. An empty drain
        // capture does not qualify a run as captured or waited for by drain.
        assert!(matches!(
            result["cases"][0]["drain_capture"].as_str(),
            Some("captured_ended") | Some("completed_without_drain_capture_evidence")
        ));
    }
}
