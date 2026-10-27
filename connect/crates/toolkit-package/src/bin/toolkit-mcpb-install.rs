//! Trusted local byte installation, not agent/runtime activation.

#[cfg(all(
    feature = "local-artifacts",
    any(target_os = "linux", target_os = "macos")
))]
mod supported {
    use serde_json::{Value, json};
    use std::collections::BTreeMap;
    use std::fs::{self, File, OpenOptions};
    use std::io::{Read, Write};
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    use std::path::{Path, PathBuf};
    use std::process::{Child, Command, Stdio};
    use std::time::{Duration, Instant};
    use toolkit_package::local_artifacts::{
        LocalArtifactInputs, LocalArtifactLayout, LocalArtifactPreparer, PreparationLimits,
        PreparedArtifactEvidence,
    };
    use toolkit_package::{
        ActivationManifest, ContentDigest, Id, IntegrationPackage, McpbInstallPlan, canonical_json,
        strict_mcpb_json,
    };

    const MAX_DOCUMENT: u64 = 1024 * 1024;
    // The bounded selected argv appears in both the plan and MCP projection.
    const MAX_READY_DOCUMENT: u64 = 8 * 1024 * 1024;
    const HELP: &str = "toolkit-mcpb-install check --package ABS --selection ABS --selection-sha256 HEX --component ID --artifact ABS --config ABS --config-sha256 HEX --destination ABS_NEW
toolkit-mcpb-install install --package ABS --selection ABS --selection-sha256 HEX --component ID --artifact ABS --config ABS --config-sha256 HEX --node ABS --destination ABS_NEW
toolkit-mcpb-install verify --package ABS --selection ABS --selection-sha256 HEX --component ID --config ABS --config-sha256 HEX --node ABS --destination ABS_EXISTING

Installs a concrete self-contained Node MCPB 0.3 selection using the existing
artifact preparation owner. Node --version is the only child command executed.
Check materializes inputs and prepared bytes in a fresh scratch destination,
emits only the canonical MCPB plan, and executes no child or runtime probe.
It publishes no ready directory and does not establish installation readiness.
The host owns all selected inputs, actual Node executable trust and directory custody.
Runtime wrappers and independently surviving child processes are unsupported.
Verify reopens and checks existing bytes; it never repairs or retries publication.
No authentication, MCP launch, grant, sandbox or native attachment is supplied.
";

    #[derive(Debug)]
    struct Failure(&'static str);
    type Result<T> = std::result::Result<T, Failure>;

    #[derive(PartialEq, Eq)]
    enum Action {
        Check,
        Install,
        Verify,
    }

    fn read(path: &Path, maximum: u64) -> Result<Vec<u8>> {
        let mut file = open(path)?;
        if file
            .metadata()
            .map_err(|_| Failure("input_unavailable"))?
            .len()
            > maximum
        {
            return Err(Failure("input_too_large"));
        }
        let mut bytes = Vec::new();
        (&mut file)
            .take(maximum + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| Failure("input_unavailable"))?;
        if bytes.len() as u64 > maximum {
            return Err(Failure("input_too_large"));
        }
        Ok(bytes)
    }

    fn open(path: &Path) -> Result<File> {
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(path)
            .map_err(|_| Failure("input_unavailable"))?;
        if !file
            .metadata()
            .map_err(|_| Failure("input_unavailable"))?
            .is_file()
        {
            return Err(Failure("regular_file_required"));
        }
        Ok(file)
    }

    fn pin(bytes: &[u8], expected: &str) -> Result<ContentDigest> {
        let value = ContentDigest::parse(expected).map_err(|_| Failure("invalid_digest"))?;
        if ContentDigest::of_bytes(bytes) != value {
            return Err(Failure("input_digest_mismatch"));
        }
        Ok(value)
    }

    fn absolute(value: &str) -> Result<PathBuf> {
        let path = PathBuf::from(value);
        if !path.is_absolute()
            || path
                .components()
                .any(|part| matches!(part, std::path::Component::ParentDir))
        {
            return Err(Failure("absolute_normalized_path_required"));
        }
        Ok(path)
    }

    fn write(path: &Path, bytes: &[u8]) -> Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(path)
            .map_err(|_| Failure("publication_unconfirmed"))?;
        file.write_all(bytes)
            .and_then(|()| file.sync_all())
            .map_err(|_| Failure("publication_unconfirmed"))
    }

    fn directory(path: &Path) -> Result<File> {
        OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
            .open(path)
            .map_err(|_| Failure("directory_unavailable"))
    }

    fn mkdir(path: &Path) -> Result<()> {
        use std::os::unix::fs::DirBuilderExt;
        fs::DirBuilder::new()
            .mode(0o700)
            .create(path)
            .map_err(|_| Failure("new_directory_required"))
    }

    struct OwnedChild(Child);
    impl Drop for OwnedChild {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    fn runtime(node: &Path, plan: &McpbInstallPlan) -> Result<Value> {
        let before = ContentDigest::of_bytes(&read(node, 256 * 1024 * 1024)?);
        let mode = open(node)?
            .metadata()
            .map_err(|_| Failure("runtime_unavailable"))?
            .permissions()
            .mode();
        if mode & 0o111 == 0 || mode & 0o7000 != 0 {
            return Err(Failure("runtime_not_executable"));
        }
        let child = Command::new(node)
            .arg("--version")
            .env_clear()
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|_| Failure("runtime_probe_failed"))?;
        let mut child = OwnedChild(child);
        let mut output = child
            .0
            .stdout
            .take()
            .ok_or(Failure("runtime_probe_failed"))?;
        let flags =
            rustix::fs::fcntl_getfl(&output).map_err(|_| Failure("runtime_probe_failed"))?;
        rustix::fs::fcntl_setfl(&output, flags | rustix::fs::OFlags::NONBLOCK)
            .map_err(|_| Failure("runtime_probe_failed"))?;
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut bytes = Vec::new();
        loop {
            if Instant::now() >= deadline {
                return Err(Failure("runtime_probe_timeout"));
            }
            let mut chunk = [0; 64];
            let eof = match output.read(&mut chunk) {
                Ok(0) => true,
                Ok(count) => {
                    bytes.extend_from_slice(&chunk[..count]);
                    false
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => false,
                Err(_) => return Err(Failure("runtime_probe_failed")),
            };
            if bytes.len() > 128 {
                return Err(Failure("runtime_probe_output_limit"));
            }
            if let Some(status) = child
                .0
                .try_wait()
                .map_err(|_| Failure("runtime_probe_failed"))?
            {
                if !status.success() {
                    return Err(Failure("runtime_probe_failed"));
                }
                if eof {
                    break;
                }
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        let version = std::str::from_utf8(&bytes)
            .map_err(|_| Failure("runtime_probe_failed"))?
            .trim_end_matches('\n');
        plan.check_node_version(version)
            .map_err(|_| Failure("unsupported_runtime"))?;
        if ContentDigest::of_bytes(&read(node, 256 * 1024 * 1024)?) != before {
            return Err(Failure("runtime_changed"));
        }
        Ok(json!({"executable":node,"sha256":before,"version":version}))
    }

    fn names(path: &Path, expected: &[&str]) -> Result<()> {
        let mut actual = Vec::new();
        for entry in fs::read_dir(path).map_err(|_| Failure("stored_inputs_unavailable"))? {
            let entry = entry.map_err(|_| Failure("stored_inputs_unavailable"))?;
            actual.push(
                entry
                    .file_name()
                    .into_string()
                    .map_err(|_| Failure("stored_inputs_changed"))?,
            );
        }
        actual.sort();
        let mut expected = expected
            .iter()
            .map(|value| value.to_string())
            .collect::<Vec<_>>();
        expected.sort();
        if actual != expected {
            return Err(Failure("stored_inputs_changed"));
        }
        Ok(())
    }

    fn run(arguments: Vec<String>) -> Result<Vec<u8>> {
        let Some(action) = arguments.first() else {
            return Err(Failure("invalid_arguments"));
        };
        let action = match action.as_str() {
            "check" => Action::Check,
            "install" => Action::Install,
            "verify" => Action::Verify,
            _ => return Err(Failure("invalid_arguments")),
        };
        let expected = if action == Action::Install { 19 } else { 17 };
        if arguments.len() != expected {
            return Err(Failure("invalid_arguments"));
        }
        let mut flags = BTreeMap::new();
        for pair in arguments[1..].chunks_exact(2) {
            if ![
                "--package",
                "--selection",
                "--selection-sha256",
                "--component",
                "--artifact",
                "--config",
                "--config-sha256",
                "--node",
                "--destination",
            ]
            .contains(&pair[0].as_str())
                || (action == Action::Verify && pair[0] == "--artifact")
                || (action == Action::Check && pair[0] == "--node")
                || flags.insert(pair[0].as_str(), pair[1].as_str()).is_some()
            {
                return Err(Failure("invalid_arguments"));
            }
        }
        let take = |key: &str| flags.get(key).copied().ok_or(Failure("invalid_arguments"));
        let package_raw = read(&absolute(take("--package")?)?, MAX_DOCUMENT)?;
        let selection_raw = read(&absolute(take("--selection")?)?, MAX_DOCUMENT)?;
        let config_raw = read(&absolute(take("--config")?)?, MAX_DOCUMENT)?;
        let selection_sha = pin(&selection_raw, take("--selection-sha256")?)?;
        pin(&config_raw, take("--config-sha256")?)?;
        let parse = |bytes: &[u8]| strict_mcpb_json(bytes).map_err(|_| Failure("invalid_document"));
        let package: IntegrationPackage =
            serde_json::from_value(parse(&package_raw)?).map_err(|_| Failure("invalid_package"))?;
        let selected: ActivationManifest = serde_json::from_value(parse(&selection_raw)?)
            .map_err(|_| Failure("invalid_selection"))?;
        if selected.configuration.content_digest != ContentDigest::of_bytes(&config_raw) {
            return Err(Failure("configuration_selection_mismatch"));
        }
        parse(&config_raw)?;
        let selected = selected
            .validate(&package)
            .map_err(|_| Failure("invalid_selection"))?;
        let component = Id::new(take("--component")?).map_err(|_| Failure("invalid_component"))?;
        McpbInstallPlan::validate_selection(&package, &selected, &component)
            .map_err(|_| Failure("unsupported_selection"))?;
        let node = if action == Action::Check {
            None
        } else {
            Some(absolute(take("--node")?)?)
        };
        let requested = absolute(take("--destination")?)?;
        let parent = requested
            .parent()
            .ok_or(Failure("invalid_destination"))?
            .canonicalize()
            .map_err(|_| Failure("destination_parent_unavailable"))?;
        let destination = parent.join(
            requested
                .file_name()
                .ok_or(Failure("invalid_destination"))?,
        );
        if destination.to_str().is_none() {
            return Err(Failure("utf8_destination_required"));
        }
        let inputs = [
            ("package.json", package_raw.as_slice()),
            ("activation.json", selection_raw.as_slice()),
            ("configuration.json", config_raw.as_slice()),
        ];
        if action != Action::Verify {
            let artifact = open(&absolute(take("--artifact")?)?)?;
            mkdir(&destination)?;
            mkdir(&destination.join("inputs"))?;
            for (name, bytes) in inputs {
                write(&destination.join("inputs").join(name), bytes)?;
            }
            directory(&destination.join("inputs"))?
                .sync_all()
                .map_err(|_| Failure("publication_unconfirmed"))?;
            let owner = LocalArtifactPreparer::new(cap_std::fs::Dir::from_std_file(directory(
                &destination,
            )?));
            owner
                .prepare(
                    &selected,
                    &component,
                    LocalArtifactLayout::Zip,
                    LocalArtifactInputs {
                        artifact,
                        pinned_dependencies: BTreeMap::new(),
                    },
                    &PreparationLimits::default(),
                )
                .map_err(|_| Failure("preparation_unconfirmed"))?;
        } else {
            directory(&destination)?;
            names(&destination, &["inputs", "prepared", "ready"])?;
            directory(&destination.join("inputs"))?;
            names(
                &destination.join("inputs"),
                &["package.json", "activation.json", "configuration.json"],
            )?;
            for (name, bytes) in inputs {
                if read(&destination.join("inputs").join(name), MAX_DOCUMENT)? != bytes {
                    return Err(Failure("stored_inputs_changed"));
                }
            }
        }
        let owner =
            LocalArtifactPreparer::new(cap_std::fs::Dir::from_std_file(directory(&destination)?));
        directory(&destination.join("prepared"))?;
        let raw = read(
            &destination.join("prepared/evidence.json"),
            16 * 1024 * 1024,
        )?;
        let evidence: PreparedArtifactEvidence = serde_json::from_value(parse(&raw)?)
            .map_err(|_| Failure("invalid_prepared_evidence"))?;
        let prepared = owner
            .reopen_verified(
                &selected,
                &component,
                &LocalArtifactLayout::Zip,
                &evidence,
                &PreparationLimits::default(),
            )
            .map_err(|_| Failure("prepared_verification_failed"))?;
        let plan = McpbInstallPlan::new(&package, &selected, &component, &prepared, &config_raw)
            .map_err(|_| Failure("unsupported_mcpb_launch"))?;
        if action == Action::Check {
            let bytes = canonical_json(&plan).map_err(|_| Failure("serialization_failed"))?;
            if bytes.len() as u64 > MAX_READY_DOCUMENT {
                return Err(Failure("launch_receipt_too_large"));
            }
            return Ok(bytes);
        }
        let node = node.ok_or(Failure("invalid_arguments"))?;
        let runtime = runtime(&node, &plan)?;
        let script = destination
            .join("prepared/tree")
            .join(&plan.entrypoint.executable);
        let mut argv = vec![
            script
                .to_str()
                .ok_or(Failure("utf8_destination_required"))?
                .to_string(),
        ];
        argv.extend(plan.entrypoint.arguments.clone());
        let mcp = json!({"mcpServers":{(plan.mcp_name.as_str()):{"command":node,"args":argv}}});
        let receipt = json!({"schema_version":1,"scope":"observed_mcpb_software_installation_only",
            "selection_sha256":selection_sha,"package_sha256":ContentDigest::of_bytes(&package_raw),
            "configuration_sha256":ContentDigest::of_bytes(&config_raw),"destination":destination,
            "plan":plan,"runtime":runtime,"mcp_configuration":mcp,
            "launch_environment":"Host-owned. No environment isolation, credential injection, tool filtering, protocol readiness or native attachment is implied."});
        let receipt_raw = canonical_json(&receipt).map_err(|_| Failure("serialization_failed"))?;
        let mcp_raw = canonical_json(&mcp).map_err(|_| Failure("serialization_failed"))?;
        if receipt_raw.len() as u64 > MAX_READY_DOCUMENT
            || mcp_raw.len() as u64 > MAX_READY_DOCUMENT
        {
            return Err(Failure("launch_receipt_too_large"));
        }
        if action == Action::Install {
            let pending = destination.join("pending-ready");
            mkdir(&pending)?;
            write(&pending.join("installed.json"), &receipt_raw)?;
            write(&pending.join("mcp.json"), &mcp_raw)?;
            directory(&pending)?
                .sync_all()
                .map_err(|_| Failure("publication_unconfirmed"))?;
            rustix::fs::renameat_with(
                &directory(&destination)?,
                "pending-ready",
                &directory(&destination)?,
                "ready",
                rustix::fs::RenameFlags::NOREPLACE,
            )
            .map_err(|_| Failure("publication_unconfirmed"))?;
            directory(&destination)?
                .sync_all()
                .map_err(|_| Failure("publication_unconfirmed"))?;
        } else {
            directory(&destination.join("ready"))?;
            names(&destination.join("ready"), &["installed.json", "mcp.json"])?;
            if read(
                &destination.join("ready/installed.json"),
                MAX_READY_DOCUMENT,
            )? != receipt_raw
                || read(&destination.join("ready/mcp.json"), MAX_READY_DOCUMENT)? != mcp_raw
            {
                return Err(Failure("launch_receipt_changed"));
            }
        }
        Ok(receipt_raw)
    }

    pub fn main() -> std::process::ExitCode {
        let args = match std::env::args_os()
            .skip(1)
            .map(|value| value.into_string())
            .collect::<std::result::Result<Vec<_>, _>>()
        {
            Ok(args) => args,
            Err(_) => {
                eprintln!("invalid_arguments");
                return std::process::ExitCode::FAILURE;
            }
        };
        if args == ["--help"] {
            print!("{HELP}");
            return std::process::ExitCode::SUCCESS;
        }
        match run(args) {
            Ok(bytes) => {
                let mut out = std::io::stdout().lock();
                if out
                    .write_all(&bytes)
                    .and_then(|()| out.write_all(b"\n"))
                    .is_err()
                {
                    eprintln!(
                        "output_unconfirmed: inspect the selected destination; do not retry into it"
                    );
                    return std::process::ExitCode::FAILURE;
                }
                std::process::ExitCode::SUCCESS
            }
            Err(error) => {
                eprintln!(
                    "{}: no readiness claim; inspect the selected destination before any new attempt",
                    error.0
                );
                std::process::ExitCode::FAILURE
            }
        }
    }
}

#[cfg(all(
    feature = "local-artifacts",
    any(target_os = "linux", target_os = "macos")
))]
fn main() -> std::process::ExitCode {
    supported::main()
}
#[cfg(not(all(
    feature = "local-artifacts",
    any(target_os = "linux", target_os = "macos")
)))]
fn main() -> std::process::ExitCode {
    eprintln!("unsupported_platform");
    std::process::ExitCode::FAILURE
}
