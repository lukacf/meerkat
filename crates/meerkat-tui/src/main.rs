//! `rkat-tui` binary entry point.

use clap::Parser;
use meerkat_tui::{Args, run};
use std::process::ExitCode;

const BINARY_NAME: &str = "rkat-tui";

fn main() -> ExitCode {
    // SAFETY: the first statement of main: no other thread exists yet and
    // nothing has used SQLite, as the switch requires.
    #[allow(unsafe_code)]
    let _ = unsafe { meerkat_sqlite::disable_memory_statistics_at_process_start() };
    // Parse before touching the terminal or the runtime so `--help`,
    // `--version` and usage errors render plainly and exit with clap's codes.
    let args = Args::parse();
    // One documented worker-stack budget for every host binary; the main
    // future runs on a budgeted thread too, not the platform main thread.
    match meerkat_runtime::host_stack::run_host(BINARY_NAME, || async move {
        match run(args).await {
            Ok(()) => ExitCode::SUCCESS,
            Err(err) => {
                eprintln!("{BINARY_NAME}: {err}");
                ExitCode::from(err.exit_code())
            }
        }
    }) {
        Ok(code) => code,
        Err(err) => {
            eprintln!("{BINARY_NAME}: {err}");
            ExitCode::from(2)
        }
    }
}
