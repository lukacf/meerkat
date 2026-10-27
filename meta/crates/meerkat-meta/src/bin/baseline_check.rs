#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::uninlined_format_args,
    clippy::collapsible_if,
    clippy::redundant_clone,
    clippy::needless_raw_string_hashes,
    clippy::single_match,
    clippy::redundant_closure_for_method_calls,
    clippy::redundant_pattern_matching,
    clippy::ignored_unit_patterns,
    clippy::clone_on_copy,
    clippy::manual_assert,
    clippy::unwrap_in_result,
    clippy::useless_vec
)]
//! Phase 0 binary — verifies Meerkat baseline symbols before runtime startup.

use std::process;

use meerkat_meta::verify_meerkat_baseline_symbols;

fn main() {
    let report = match verify_meerkat_baseline_symbols(None) {
        Ok(report) => report,
        Err(err) => {
            eprintln!("baseline validation failed: {err}");
            process::exit(1);
        }
    };

    println!(
        "repo={} missing_symbols={}",
        report.repo_root.display(),
        report.missing_symbols.len()
    );
}
