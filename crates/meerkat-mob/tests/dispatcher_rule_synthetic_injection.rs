//! Tripwire for wave-c (Section 1.5 #6). Flipped green by the c.1
//! exit gate (negative-case for the B-10 `CompositionDispatchIsThePath`
//! rule at `xtask/src/rmat_audit.rs:488+`).
//!
//! Invariant: the B-10 rule is not silently vacuous. If we delete a
//! `dispatcher.dispatch(...)` call from a fixture, the rule must
//! report a violation. This catches the false-green where the rule
//! still runs but stops matching anything real.
//!
//! Method: create a tempdir copy of a minimal fixture source,
//! mechanically remove a known `dispatcher.dispatch(...)` call, invoke
//! `cargo run -p xtask -- rmat-audit --rule CompositionDispatchIsThePath`
//! against the tempdir, assert the exit code is non-zero OR the output
//! contains a violation for the removed site, then restore.
//!
//! Status at c.0: marked `#[ignore]` — the invocation harness for
//! `rmat-audit` against a tempdir requires either an xtask flag that
//! accepts a root path, or a library-level entry point to the rule.
//! Neither exists yet. This test is the commitment to land both and
//! the ignore tag is dropped at the c.1 exit gate.

#![allow(clippy::unwrap_used, clippy::expect_used)]

#[test]
#[ignore = "un-ignore at c.1 exit gate: requires rmat-audit to accept \
            an alternate root (xtask CLI flag or library entry point) \
            so we can point it at a tempdir with a removed \
            `dispatcher.dispatch(...)` call"]
fn rule_reports_violation_when_dispatch_call_is_removed() {
    // TODO(c.1): once xtask exposes either
    //   (a) `xtask rmat-audit --root <path> --rule <RuleName>`
    //   (b) `xtask::rmat_audit::run_rule(RuleName, &Path) -> Findings`
    // …do the following:
    //   1. tempdir with a known-good fixture containing a
    //      `dispatcher.dispatch(...)` call.
    //   2. rewrite the file with that call mechanically removed.
    //   3. invoke the rule; assert the `Findings` list is non-empty
    //      and names the modified file.
    //   4. assert the positive baseline (pre-removal) returned empty.
    //
    // The invariant is the symmetric proof: rule green on baseline,
    // red after excision.
    unreachable!("rmat-audit negative-case tripwire: un-ignore at c.1 exit gate");
}
