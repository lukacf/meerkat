//! Local Toolkit owner plus the ordinary MobKit persistent SDK gateway.

#[cfg(unix)]
fn main() -> std::process::ExitCode {
    use meerkat_connect::application_gateway::InheritedOperatorTerminal;
    use meerkat_mobkit::gateway_composition::gateway_tokio_runtime;
    use std::sync::Arc;
    use toolkit_mobkit::private_host::{
        InheritedHostPipes, PrivateHostStartup, PrivateHostStartupIntent,
    };

    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(result) = meerkat_connect::application_bootstrap::run(&args) {
        return result;
    }
    if args.as_slice() == ["--version"] || args.as_slice() == ["-V"] {
        println!(
            "toolkit_gateway {} (local Toolkit host over MobKit SDK RPC)",
            env!("CARGO_PKG_VERSION")
        );
        return std::process::ExitCode::SUCCESS;
    }
    let (
        read_fd,
        write_fd,
        control_listen,
        startup,
        operator_fd,
        private_workflows,
        hosted_mcp_redirect_base,
    ) = match parse_args(&args) {
        Ok(args) => args,
        Err(error) => {
            eprintln!("toolkit_gateway: {error}");
            return std::process::ExitCode::from(2);
        }
    };
    // This optional input is transferred by the application composition, not
    // by public RPC/bootstrap JSON. Adopt before Tokio or any child can spawn.
    let operator_terminal = match operator_fd
        .map(|fd| InheritedOperatorTerminal::adopt(fd, read_fd, write_fd))
        .transpose()
    {
        Ok(terminal) => terminal,
        Err(error) => {
            eprintln!("toolkit_gateway: {error}");
            return std::process::ExitCode::FAILURE;
        }
    };
    // Set close-on-exec and close original inherited descriptors before the
    // runtime, tracing, SDK bootstrap or any tool process can create children.
    let pipes = match InheritedHostPipes::adopt(read_fd, write_fd) {
        Ok(pipes) => pipes,
        Err(error) => {
            eprintln!("toolkit_gateway: {error}");
            return std::process::ExitCode::FAILURE;
        }
    };
    let runtime = match gateway_tokio_runtime() {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("toolkit_gateway: {error}");
            return std::process::ExitCode::FAILURE;
        }
    };
    let result: Result<(), String> = runtime.block_on(async move {
        let private_startup = PrivateHostStartup::read(pipes).await.map_err(|error| error.to_string())?;
        if matches!(private_startup.intent(), PrivateHostStartupIntent::PrepareApplication(_)) {
            if startup != toolkit_mobkit::installed_host::NativeOwnerStartup::Restore {
                return Err("application preparation selects initialize or restore in its trusted plan".into());
            }
            return meerkat_connect::application_driver::run_with_options(
                private_startup, operator_terminal, control_listen,
                meerkat_connect::application_driver::ApplicationDriverOptions {
                    operation_review_policy: None,
                    workflows: private_workflows.then(|| Arc::new(
                        meerkat_connect::application_acquisition::workflow::PrivateApplicationWorkflowLauncher::default())
                        as Arc<dyn meerkat_connect::application_acquisition::workflow::ApplicationWorkflowLaunchOwner>),
                    hosted_mcp_redirect_base,
                    operation_review: None, abandoned_workflows: None,
                },
            ).await;
        }
        meerkat_connect::application_installed::run_with_options(
            private_startup, operator_terminal, control_listen,
            meerkat_connect::application_installed::InstalledGatewayOptions {
                operation_review_policy: None,
                startup, private_workflows, hosted_mcp_redirect_base, operation_review: None, console_consent: None, abandoned_workflows: None,
            },
        ).await
    });
    // All owned cleanup above has returned. Tokio's uncancellable stdin read
    // is an I/O helper, not a native effect whose retirement is inferred here.
    runtime.shutdown_background();
    match result {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("toolkit_gateway: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}

/// The gateway's parsed arguments: its two descriptors, the control listen
/// address, the native owner startup, the optional operator terminal
/// descriptor, the private workflow launch flag and the hosted MCP redirect
/// base.
#[cfg(unix)]
type GatewayArgs = (
    i32,
    i32,
    Option<meerkat_mobkit::runtime::cross_mob_control::ControlListenAddr>,
    toolkit_mobkit::installed_host::NativeOwnerStartup,
    Option<i32>,
    bool,
    Option<meerkat::McpRegisteredRedirectBase>,
);

#[cfg(unix)]
fn parse_args(args: &[String]) -> Result<GatewayArgs, &'static str> {
    use meerkat_mobkit::runtime::cross_mob_control::ControlListenAddr;
    let mut read_fd = None;
    let mut write_fd = None;
    let mut control = None;
    let mut persistent = false;
    let mut initialize = false;
    let mut operator_fd = None;
    let mut private_workflows = false;
    let mut hosted_mcp_redirect_base = None;
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--persistent" if !persistent => persistent = true,
            "--initialize-native-owner" if !initialize => initialize = true,
            "--private-workflow-launch" if !private_workflows => private_workflows = true,
            "--hosted-mcp-redirect-base" if hosted_mcp_redirect_base.is_none() => {
                hosted_mcp_redirect_base = Some(
                    meerkat::McpRegisteredRedirectBase::parse(
                        args.next().ok_or("missing hosted MCP redirect base")?,
                    )
                    .map_err(|_| "invalid hosted MCP redirect base")?,
                );
            }
            "--operator-terminal-fd" if operator_fd.is_none() => {
                operator_fd = Some(
                    args.next()
                        .ok_or("missing inherited operator terminal descriptor")?
                        .parse::<i32>()
                        .map_err(|_| "invalid inherited operator terminal descriptor")?,
                );
            }
            "--host-control-read-fd" if read_fd.is_none() => {
                read_fd = Some(
                    args.next()
                        .ok_or("missing inherited read descriptor")?
                        .parse::<i32>()
                        .map_err(|_| "invalid inherited read descriptor")?,
                );
            }
            "--host-control-write-fd" if write_fd.is_none() => {
                write_fd = Some(
                    args.next()
                        .ok_or("missing inherited write descriptor")?
                        .parse::<i32>()
                        .map_err(|_| "invalid inherited write descriptor")?,
                );
            }
            "--control-listen" if control.is_none() => {
                control = Some(
                    ControlListenAddr::parse(
                        args.next().ok_or("missing control listener address")?,
                    )
                    .map_err(|_| "invalid control listener address")?,
                );
            }
            _ => return Err("unknown or repeated Toolkit gateway argument"),
        }
    }
    if !persistent {
        return Err("Toolkit gateway requires --persistent");
    }
    let read_fd = read_fd.ok_or("Toolkit gateway requires a private inherited read descriptor")?;
    let write_fd =
        write_fd.ok_or("Toolkit gateway requires a private inherited write descriptor")?;
    if read_fd < 3 || write_fd < 3 || read_fd == write_fd {
        return Err("invalid private inherited descriptors");
    }
    if operator_fd.is_some_and(|fd| fd < 3 || fd == read_fd || fd == write_fd) {
        return Err("operator terminal collides with RPC or private descriptors");
    }
    Ok((
        read_fd,
        write_fd,
        control,
        if initialize {
            toolkit_mobkit::installed_host::NativeOwnerStartup::Initialize
        } else {
            toolkit_mobkit::installed_host::NativeOwnerStartup::Restore
        },
        operator_fd,
        private_workflows,
        hosted_mcp_redirect_base,
    ))
}

#[cfg(not(unix))]
fn main() -> std::process::ExitCode {
    eprintln!("toolkit_gateway: private inherited host control requires a POSIX host");
    std::process::ExitCode::FAILURE
}

#[cfg(all(test, unix))]
mod tests {
    use super::parse_args;
    fn args(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| (*item).into()).collect()
    }
    #[test]
    fn hosted_mcp_base_is_explicit_validated_and_never_echoed() {
        let base = args(&[
            "--persistent",
            "--host-control-read-fd",
            "3",
            "--host-control-write-fd",
            "4",
        ]);
        assert!(parse_args(&base).unwrap().6.is_none());
        let mut valid = base.clone();
        valid.extend(args(&[
            "--hosted-mcp-redirect-base",
            "https://console.example/host/toolkit/mcp/callback",
        ]));
        let selected = parse_args(&valid).unwrap();
        assert!(!selected.5, "hosted auth does not select shared workflows");
        assert_eq!(
            selected.6.unwrap().as_str(),
            "https://console.example/host/toolkit/mcp/callback"
        );
        valid.extend(args(&[
            "--hosted-mcp-redirect-base",
            "https://other.example/callback",
        ]));
        assert_eq!(
            parse_args(&valid).err(),
            Some("unknown or repeated Toolkit gateway argument")
        );
        let mut missing = base.clone();
        missing.push("--hosted-mcp-redirect-base".into());
        assert_eq!(
            parse_args(&missing).err(),
            Some("missing hosted MCP redirect base")
        );
        for value in [
            "",
            "http://console.example/callback",
            "/callback",
            "https://private:secret@console.example/callback",
            "https://console.example/callback?private=secret",
            "https://console.example/callback#private",
        ] {
            let mut invalid = base.clone();
            invalid.extend(args(&["--hosted-mcp-redirect-base", value]));
            assert_eq!(
                parse_args(&invalid).err(),
                Some("invalid hosted MCP redirect base")
            );
        }
    }

    #[test]
    fn private_workflow_launch_is_explicit_and_not_repeatable() {
        let mut values = args(&[
            "--persistent",
            "--host-control-read-fd",
            "3",
            "--host-control-write-fd",
            "4",
        ]);
        assert!(!parse_args(&values).unwrap().5);
        values.push("--private-workflow-launch".into());
        assert!(parse_args(&values).unwrap().5);
        values.push("--private-workflow-launch".into());
        assert!(parse_args(&values).is_err());
    }
    #[test]
    fn operator_terminal_is_optional_and_cannot_reuse_existing_descriptors() {
        let base = args(&[
            "--persistent",
            "--host-control-read-fd",
            "3",
            "--host-control-write-fd",
            "4",
        ]);
        assert_eq!(parse_args(&base).unwrap().4, None);
        for descriptor in ["0", "1", "2", "3", "4", "-1"] {
            let mut values = base.clone();
            values.extend(args(&["--operator-terminal-fd", descriptor]));
            assert!(parse_args(&values).is_err());
        }
        let mut values = base;
        values.extend(args(&["--operator-terminal-fd", "5"]));
        assert_eq!(parse_args(&values).unwrap().4, Some(5));
        values.extend(args(&["--operator-terminal-fd", "6"]));
        assert!(parse_args(&values).is_err());
    }
    #[test]
    fn native_initialization_is_an_explicit_private_launch_choice() {
        use toolkit_mobkit::installed_host::NativeOwnerStartup;
        let mut values = args(&[
            "--persistent",
            "--host-control-read-fd",
            "3",
            "--host-control-write-fd",
            "4",
        ]);
        assert_eq!(parse_args(&values).unwrap().3, NativeOwnerStartup::Restore);
        values.push("--initialize-native-owner".into());
        assert_eq!(
            parse_args(&values).unwrap().3,
            NativeOwnerStartup::Initialize
        );
        values.push("--initialize-native-owner".into());
        assert!(parse_args(&values).is_err());
    }
    #[test]
    fn public_rpc_flags_cannot_select_an_unowned_gateway() {
        for value in [
            vec![],
            vec!["--persistent"],
            vec![
                "--persistent",
                "--host-control-read-fd",
                "0",
                "--host-control-write-fd",
                "1",
            ],
            vec![
                "--persistent",
                "--host-control-read-fd",
                "3",
                "--host-control-write-fd",
                "4",
                "--owner",
                "allow",
            ],
        ] {
            assert!(parse_args(&args(&value)).is_err());
        }
        assert!(
            parse_args(&args(&[
                "--persistent",
                "--host-control-read-fd",
                "3",
                "--host-control-write-fd",
                "4"
            ]))
            .is_ok()
        );
    }
}
