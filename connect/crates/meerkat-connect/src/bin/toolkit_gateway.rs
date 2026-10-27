//! Local Toolkit owner plus the ordinary MobKit persistent SDK gateway.

#[cfg(unix)]
fn main() -> std::process::ExitCode {
    use meerkat_mobkit::{
        gateway_composition::gateway_tokio_runtime, rpc_gateway::run_persistent_with_host,
    };
    use std::sync::Arc;
    use meerkat_connect::application_gateway::{InheritedOperatorTerminal, serve_local_operator, serve_local_operator_with_setup};
    use meerkat_connect::application_remote::serve_remote_operator_with_service_parent;
    use toolkit_mobkit::{
        private_host::{InheritedHostPipes, PrivateHostConnection, PrivateHostStartup, PrivateHostStartupIntent},
    };

    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(result) = meerkat_connect::application_bootstrap::run(&args) { return result; }
    if args.as_slice() == ["--version"] || args.as_slice() == ["-V"] {
        println!(
            "toolkit_gateway {} (local Toolkit host over MobKit SDK RPC)",
            env!("CARGO_PKG_VERSION")
        );
        return std::process::ExitCode::SUCCESS;
    }
    let (read_fd, write_fd, control_listen, startup, operator_fd) = match parse_args(&args) {
        Ok(args) => args,
        Err(error) => {
            eprintln!("toolkit_gateway: {error}");
            return std::process::ExitCode::from(2);
        }
    };
    // This optional input is transferred by the application composition, not
    // by public RPC/bootstrap JSON. Adopt before Tokio or any child can spawn.
    let operator_terminal = match operator_fd.map(|fd| InheritedOperatorTerminal::adopt(fd, read_fd, write_fd)).transpose() {
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
            return meerkat_connect::application_driver::run(private_startup, operator_terminal, control_listen).await;
        }
        // The existing transaction's trusted launcher selects replacement input.
        // Validate before restoring an owner; no no-key fallback or fresh setup.
        let replacement = match private_startup.intent() {
            PrivateHostStartupIntent::ReplaceApplication(selected) => {
                if startup != toolkit_mobkit::installed_host::NativeOwnerStartup::Restore {
                    return Err("replacement application startup requires original-owner Restore".into());
                }
                match meerkat_connect::application_update::ValidatedReplacementStartup::validate(selected) {
                    Ok(selected) => Some(selected),
                    Err(error) => {
                        private_startup.reject(error).await.map_err(|error| error.to_string())?;
                        return Err("replacement application inputs were refused".into());
                    }
                }
            }
            _ => None,
        };
        let private_startup = match &replacement {
            Some(selected) => private_startup.replace_with_original(selected.predecessor().clone())
                .map_err(|error| error.to_string())?,
            None => private_startup,
        };
        let mut composition = None;
        let mut acquisition_composition = None;
        let mut operator_artifacts = None;
        let (owner, _, private, operator_reviews, remote_reviews) =
            PrivateHostConnection::bootstrap_from_startup_with_controller_reviews(private_startup, |selection| {
                let composition = &mut composition;
                let acquisition_composition = &mut acquisition_composition;
                let operator_artifacts = &mut operator_artifacts;
                async move {
                    *operator_artifacts = Some(selection.deployment_root.clone());
                    let (owner, installed, acquisition) = match replacement {
                        Some(selected) => selected.open().await?,
                        None => {
                            meerkat_connect::application_acquisition::open_gateway_acquisition(
                                selection.clone(), startup).await?
                        }
                    };
                    *acquisition_composition = acquisition;
                    let originals = installed.originals();
                    *composition = Some(installed);
                    Ok((owner, originals))
                }
            })
                .await
                .map_err(|error| error.to_string())?;
        let service_parent = match private.service_parent_endpoint() {
            Ok(endpoint) => endpoint,
            Err(error) => {
                let cleanup = private.finish().await;
                return Err(match cleanup {
                    Ok(()) => error.to_string(),
                    Err(cleanup) => format!("{error}; private host cleanup also failed: {cleanup}"),
                });
            }
        };
        // The FnOnce factory above either returns both objects or bootstrap
        // fails. It cannot select the legacy ownerless constructor.
        let installed = match composition {
            Some(installed) => installed,
            None => {
                let cleanup = private.finish().await;
                return Err(match cleanup {
                    Ok(()) => "installed Toolkit composition is unavailable".into(),
                    Err(error) => format!("installed Toolkit composition is unavailable; private host cleanup also failed: {error}"),
                });
            }
        };
        // The existing transferred terminal owns this enrollment receiver.
        // Create it before native init so every offered factory can enroll its
        // actual selected adapter. No terminal means no local setup presenter.
        let (setup_approvals, setup_reviews) = match (&operator_terminal, &operator_artifacts) {
            (Some(terminal), Some(parent)) if acquisition_composition.is_some() => {
                let (approvals, reviews) = terminal.setup_reviews(parent.clone());
                (Some(approvals), Some(reviews))
            }
            _ => (None, None),
        };
        let host = match installed.compose(owner.clone(), private.closed_observer())
            .with_console_logins(private.console_logins()) {
            Ok(host) => host,
            Err(error) => {
                let cleanup = private.finish().await;
                return Err(match cleanup {
                    Ok(()) => error.to_string(),
                    Err(cleanup) => format!("{error}; private host cleanup also failed: {cleanup}"),
                });
            }
        };
        let mut console_setup_reviews = None;
        let host = match acquisition_composition {
            Some(acquisition) => {
                let composed = operator_artifacts.clone()
                    .ok_or(meerkat_toolkit::host_lifetime::control::HostLifetimeError::InvalidSelection)
                    .and_then(|artifacts| acquisition.compose_selected_with_service_parent(host, owner.clone(), setup_approvals, artifacts,
                        Some(service_parent.clone())));
                match composed {
                    Ok((host, reviews)) => { console_setup_reviews = reviews; host },
                    Err(error) => {
                        let cleanup = private.finish().await;
                        return Err(match cleanup {
                            Ok(()) => error.to_string(),
                            Err(cleanup) => format!("{error}; private host cleanup also failed: {cleanup}"),
                        });
                    }
                }
            },
            None => host,
        };
        let host = Arc::new(host);
        let console_setup_task = console_setup_reviews.map(|reviews| tokio::spawn(
            meerkat_connect::application_gateway::serve_console_setup(reviews, private.closed_observer())));
        let remote_task = match operator_artifacts.clone() {
            Some(parent) => Some(tokio::spawn(serve_remote_operator_with_service_parent(
                host.clone(), remote_reviews, parent, private.closed_observer(),
                service_parent))),
            None => { drop(remote_reviews); None }
        };
        let operator_task = match (operator_terminal, operator_artifacts) {
            (Some(terminal), Some(parent)) => {
                let operator_host = host.clone();
                let closed = private.closed_observer();
                Some(tokio::spawn(async move {
                    match setup_reviews {
                        Some(setup) => serve_local_operator_with_setup(
                            operator_host, operator_reviews, terminal, parent, closed, setup).await,
                        None => serve_local_operator(
                            operator_host, operator_reviews, terminal, parent, closed).await,
                    }
                }))
            }
            _ => {
                // Without the transferred terminal this process has no local
                // operator presenter. Requests refuse, never select a fallback.
                drop(operator_reviews);
                None
            }
        };
        let gateway_result = run_persistent_with_host(control_listen, host).await;
        // Even a failed gateway initialization closes and joins the exact
        // private owner. No ordinary error path exits the process first.
        let private_result = private.finish().await;
        // The private JoinSet and this presenter join preserve issuance custody
        // even after RPC or parent loss. Neither observer aborts native work.
        let operator_result = match operator_task {
            Some(task) => task.await.map_err(|error| error.to_string())
                .and_then(|result| result.map_err(|error| error.to_string())),
            None => Ok(()),
        };
        let console_setup_result = match console_setup_task {
            Some(task) => task.await.map_err(|error| error.to_string()).and_then(|result| result.map_err(|error| error.to_string())),
            None => Ok(()),
        };
        let remote_result = match remote_task {
            Some(task) => task.await.map_err(|error| error.to_string())
                .and_then(|result| result.map_err(|error| error.to_string())),
            None => Ok(()),
        };
        let operator_result = match (operator_result, remote_result) {
            (Ok(()), Ok(())) => Ok(()),
            (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
            (Err(local), Err(remote)) => Err(format!("{local}; remote presenter cleanup also failed: {remote}")),
        };
        let operator_result = match (operator_result, console_setup_result) {
            (Ok(()), Ok(())) => Ok(()), (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
            (Err(error), Err(console)) => Err(format!("{error}; Console setup cleanup also failed: {console}")),
        };
        let result = match (gateway_result, private_result) {
            (Ok(()), Ok(())) => Ok(()),
            (Err(error), Ok(())) => Err(error.to_string()),
            (Ok(()), Err(error)) => Err(error.to_string()),
            (Err(gateway), Err(cleanup)) => Err(format!(
                "{gateway}; private host cleanup also failed: {cleanup}"
            )),
        };
        match (result, operator_result) {
            (Ok(()), Ok(())) => Ok(()),
            (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
            (Err(error), Err(operator)) => Err(format!("{error}; operator presenter cleanup also failed: {operator}")),
        }
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

#[cfg(unix)]
fn parse_args(
    args: &[String],
) -> Result<
    (
        i32,
        i32,
        Option<meerkat_mobkit::runtime::cross_mob_control::ControlListenAddr>,
        toolkit_mobkit::installed_host::NativeOwnerStartup,
        Option<i32>,
    ),
    &'static str,
> {
    use meerkat_mobkit::runtime::cross_mob_control::ControlListenAddr;
    let mut read_fd = None;
    let mut write_fd = None;
    let mut control = None;
    let mut persistent = false;
    let mut initialize = false;
    let mut operator_fd = None;
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--persistent" if !persistent => persistent = true,
            "--initialize-native-owner" if !initialize => initialize = true,
            "--operator-terminal-fd" if operator_fd.is_none() => {
                operator_fd = Some(args.next().ok_or("missing inherited operator terminal descriptor")?
                    .parse::<i32>().map_err(|_| "invalid inherited operator terminal descriptor")?);
            },
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
    fn operator_terminal_is_optional_and_cannot_reuse_existing_descriptors() {
        let base = args(&["--persistent", "--host-control-read-fd", "3", "--host-control-write-fd", "4"]);
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
