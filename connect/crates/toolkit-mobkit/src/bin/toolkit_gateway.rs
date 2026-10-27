//! Local Toolkit owner plus the ordinary MobKit persistent SDK gateway.

#[cfg(unix)]
fn main() -> std::process::ExitCode {
    use meerkat_mobkit::{
        gateway_composition::gateway_tokio_runtime, rpc_gateway::run_persistent_with_host,
    };
    use meerkat_toolkit::host_lifetime::NativeHostLifetime;
    use std::sync::Arc;
    use toolkit_mobkit::{
        host_gateway::ToolkitGatewayHost,
        private_host::{InheritedHostPipes, PrivateHostConnection},
    };

    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.as_slice() == ["--version"] || args.as_slice() == ["-V"] {
        println!(
            "toolkit_gateway {} (local Toolkit host over MobKit SDK RPC)",
            env!("CARGO_PKG_VERSION")
        );
        return std::process::ExitCode::SUCCESS;
    }
    let (read_fd, write_fd, control_listen) = match parse_args(&args) {
        Ok(args) => args,
        Err(error) => {
            eprintln!("toolkit_gateway: {error}");
            return std::process::ExitCode::from(2);
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
        let (owner, private) =
            PrivateHostConnection::bootstrap(pipes, NativeHostLifetime::bootstrap)
                .await
                .map_err(|error| error.to_string())?;
        let host = Arc::new(ToolkitGatewayHost::new(owner, private.closed_observer()));
        let gateway_result = run_persistent_with_host(control_listen, host).await;
        // Even a failed gateway initialization closes and joins the exact
        // private owner. No ordinary error path exits the process first.
        let private_result = private.finish().await;
        match (gateway_result, private_result) {
            (Ok(()), Ok(())) => Ok(()),
            (Err(error), Ok(())) => Err(error.to_string()),
            (Ok(()), Err(error)) => Err(error.to_string()),
            (Err(gateway), Err(cleanup)) => Err(format!(
                "{gateway}; private host cleanup also failed: {cleanup}"
            )),
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
    ),
    &'static str,
> {
    use meerkat_mobkit::runtime::cross_mob_control::ControlListenAddr;
    let mut read_fd = None;
    let mut write_fd = None;
    let mut control = None;
    let mut persistent = false;
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--persistent" if !persistent => persistent = true,
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
    Ok((read_fd, write_fd, control))
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
