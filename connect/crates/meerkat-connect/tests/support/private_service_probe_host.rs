//! Acceptance-only executable for the real public private-pipe entrypoint.
//! Exact binary and resource manifest are external required inputs. This must
//! never be installed as the stock product or used as a Forge login adapter.
#[cfg(unix)]
#[allow(
    dead_code,
    reason = "shared with the acceptance tests, which use the rest of the contract"
)]
#[path = "private_service_probe_contract.rs"]
mod contract;
#[cfg(unix)]
fn main() {
    let result = (|| -> Result<(), Box<dyn std::error::Error>> {
        let mut args = std::env::args_os().skip(1);
        if args.next().as_deref() != Some(std::ffi::OsStr::new("--acceptance-binding")) {
            return Err("explicit acceptance binding required".into());
        }
        let path = std::path::PathBuf::from(args.next().ok_or("missing acceptance binding")?);
        if args.next().is_some() {
            return Err("unexpected host arguments".into());
        }
        let binding = contract::Launch::read(&path)?;
        let selection = binding.selection()?;
        let control = selection.control();
        // The same test-owned native File backend policy is used throughout
        // setup, finalization and captured-factory reopen. No process env write.
        let store = binding.root.join("credential-store");
        let run_result = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?
            .block_on(meerkat_connect::stdio::run_with_service_probes(
                selection,
                Some(store),
            ));
        // Acceptance-only local evidence, after the existing runtime owner
        // returns, including Err. It cannot grant success or change that result.
        let observation = control.private_setup_diagnostic();
        let retained = serde_json::json!({"schema":"private-setup-diagnostic-v1",
            "observed":observation.is_some(), "observation":observation});
        let saved = (|| -> Result<(), Box<dyn std::error::Error>> {
            use std::io::Write;
            use std::os::unix::fs::OpenOptionsExt;
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(binding.root.join("private-setup-diagnostic.json"))?;
            file.write_all(&serde_json::to_vec(&retained)?)?;
            Ok(())
        })();
        if saved.is_err() {
            eprintln!("private setup diagnostic could not be retained");
        }
        run_result?;
        Ok(())
    })();
    // Private native errors can contain state. Return only the exit status;
    // the protected protocol carries its own redacted observations.
    if result.is_err() {
        std::process::exit(1);
    }
}
#[cfg(not(unix))]
fn main() {
    std::process::exit(2);
}
