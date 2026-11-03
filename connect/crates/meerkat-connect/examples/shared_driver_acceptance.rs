//! Acceptance-only embedding of the shipping application Driver. This is not
//! the stock toolkit_gateway CLI and adds no selectable product workflow mode.
//! The normal SDK still owns the private descriptors and native RPC child.
#[cfg(all(unix, feature = "console"))]
#[path = "shared_driver_acceptance/owner.rs"]
mod owner;

#[cfg(all(unix, feature = "console"))]
fn main() -> std::process::ExitCode {
    use meerkat_mobkit::{
        gateway_composition::gateway_tokio_runtime, runtime::cross_mob_control::ControlListenAddr,
    };
    use toolkit_mobkit::private_host::{
        InheritedHostPipes, PrivateHostStartup, PrivateHostStartupIntent,
    };
    let args: Vec<String> = std::env::args().skip(1).collect();
    // Reuse the actual artifact transaction invoked by the SDK finalizer.
    if let Some(result) = meerkat_connect::application_bootstrap::run(&args) {
        return result;
    }
    let parse = || -> Result<_, String> {
        let mut read = None;
        let mut write = None;
        let mut control = None;
        let mut persistent = false;
        let mut values = args.iter();
        while let Some(flag) = values.next() {
            match flag.as_str() {
                "--persistent" if !persistent => persistent = true,
                "--host-control-read-fd" if read.is_none() => {
                    read = Some(
                        values
                            .next()
                            .ok_or("missing private read descriptor")?
                            .parse::<i32>()
                            .map_err(|_| "invalid private read descriptor")?,
                    );
                }
                "--host-control-write-fd" if write.is_none() => {
                    write = Some(
                        values
                            .next()
                            .ok_or("missing private write descriptor")?
                            .parse::<i32>()
                            .map_err(|_| "invalid private write descriptor")?,
                    );
                }
                "--control-listen" if control.is_none() => {
                    control = Some(
                        ControlListenAddr::parse(values.next().ok_or("missing control address")?)
                            .map_err(|e| e.to_string())?,
                    );
                }
                _ => return Err("unexpected acceptance-host argument".into()),
            }
        }
        let read = read.ok_or("private read descriptor required")?;
        let write = write.ok_or("private write descriptor required")?;
        if !persistent || read < 3 || write < 3 || read == write {
            return Err("private SDK descriptors and persistent mode required".into());
        }
        Ok((read, write, control))
    };
    let result = (|| -> Result<(), String> {
        let (read, write, control) = parse()?;
        // Transfer descriptors before Tokio or any tool process can spawn.
        let pipes = InheritedHostPipes::adopt(read, write).map_err(|e| e.to_string())?;
        let runtime = gateway_tokio_runtime().map_err(|e| e.to_string())?;
        let result = runtime.block_on(async move {
            let startup = PrivateHostStartup::read(pipes)
                .await
                .map_err(|e| e.to_string())?;
            let PrivateHostStartupIntent::PrepareApplication(reference) = startup.intent() else {
                return Err("this acceptance requires the real fresh application Driver".into());
            };
            let owner = owner::LaunchOwner::load(reference)?;
            let task_owner = owner.clone();
            let task = tokio::spawn(async move { task_owner.exercise().await });
            let driver = meerkat_connect::application_driver::run_with_workflows(
                startup,
                None,
                control,
                owner.clone(),
            )
            .await;
            owner.stop();
            // The application launch operation stays owned across lost test
            // observation. Native shutdown remains entirely with the Driver.
            let application = task.await.map_err(|e| e.to_string());
            let released = owner.release_runtime();
            driver.and(application?).and(released)
        });
        // Match the shipped entry's treatment of Tokio's uncancellable stdin
        // helper after actual Driver/native/private shutdown has returned.
        runtime.shutdown_background();
        result
    })();
    match result {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("shared-driver-acceptance: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}

#[cfg(not(all(unix, feature = "console")))]
fn main() {
    eprintln!("shared_driver_acceptance requires Unix and the console feature");
    std::process::exit(2);
}
