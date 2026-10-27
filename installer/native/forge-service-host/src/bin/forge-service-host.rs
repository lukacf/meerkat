#![deny(unsafe_code)]

#[cfg(target_os = "linux")]
#[allow(unsafe_code)]
#[path = "forge-service-host/startup.rs"]
mod startup;

#[cfg(target_os = "linux")]
fn main() -> std::process::ExitCode {
    use forge_service_host::{ForgeGheCatalog, HistoricalStore};
    use meerkat_connect::protected_parent::ProtectedServiceInputs;
    use std::sync::Arc;

    // First application action: no stdin/stdout wrapper, logger, runtime or
    // helper may exist while startup acquires these original inherited FDs.
    let Ok(original) = startup::take_originals_at_entry(std::env::args_os().skip(1)) else {
        return std::process::ExitCode::FAILURE;
    };
    let Ok(inputs) = ProtectedServiceInputs::adopt(
        original.parent,
        original.mapping,
        original.input,
        original.output,
    ) else {
        return std::process::ExitCode::FAILURE;
    };
    let history = Arc::new(HistoricalStore::default());
    let catalog = Arc::new(ForgeGheCatalog::new(history.clone()));
    let Ok(runtime) = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    else {
        return std::process::ExitCode::FAILURE;
    };
    let closed = runtime.block_on(async { inputs.start(catalog.clone())?.join().await });
    // Outside native callbacks and after original close, retain completed parser
    // rows even if the native final selection recheck or close returned failure.
    // Root must separately match the SDK-validated report before consumption.
    let archived = catalog.archive_after_close();
    drop(history);
    if closed.is_ok() && archived.is_ok() {
        std::process::ExitCode::SUCCESS
    } else {
        std::process::ExitCode::FAILURE
    }
}

#[cfg(not(target_os = "linux"))]
fn main() -> std::process::ExitCode {
    // There is no unprotected fallback on another platform.
    std::process::ExitCode::FAILURE
}
