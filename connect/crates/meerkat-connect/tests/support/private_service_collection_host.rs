//! Acceptance-only executable for the real public private-pipe entrypoint.
//! Exact binary and resource manifest are external required inputs. This must
//! never be installed as the stock product or used as a Forge login adapter.
#[cfg(unix)]
#[allow(
    dead_code,
    reason = "shared with the acceptance tests, which use the rest of the contract"
)]
#[path = "private_service_collection_contract.rs"]
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
        // The same test-owned native File backend policy is used throughout
        // setup, finalization and captured-factory reopen. No process env write.
        let store = binding.root.join("credential-store");
        // Match Core's host worker budget: the native acceptance path exceeds
        // Tokio's default 2 MiB stack in debug builds. Keep the fixture env fixed.
        let result = tokio::runtime::Builder::new_multi_thread()
            .thread_stack_size(meerkat_runtime::host_stack::HOST_WORKER_STACK_BUDGET)
            .enable_all()
            .build()?
            .block_on(meerkat_connect::stdio::run_with_service_collections(
                selection,
                Some(store),
            ));
        if let Err(error) = result {
            // Actual returned Rust error custody, not a received pipe frame.
            // Keep the original error alive through this private artifact write.
            let evidence = (|| -> Result<(), Box<dyn std::error::Error>> {
                if let meerkat_connect::ConnectError::Native { source, .. } = &error
                    && let Some(retained)=source.downcast_ref::<meerkat_connect::service_collection::ServiceCollectionFailure>() {
                    use std::io::Write;use std::os::unix::fs::OpenOptionsExt;
                    let path=binding.root.join("returned-collection-failure.json");
                    let mut file=std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(path)?;
                    let bytes=serde_json::to_vec_pretty(&serde_json::json!({"reports":retained.reports(),
                        "delivery":"not_claimed","original_error_retained":true}))?;
                    file.write_all(&bytes)?;file.sync_all()?;std::fs::File::open(&binding.root)?.sync_all()?;
                    let _=retained.original_error();
                }
                Ok(())
            })();
            if evidence.is_err() {
                eprintln!(
                    "collection failure evidence write failed; original native error remains retained"
                );
            }
            return Err(Box::new(error));
        }
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
