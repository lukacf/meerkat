#![allow(clippy::expect_used)]

const LIVE_WORKSPACE_RUNFILES: &str = "required";

#[test]
fn live_workspace_rmat_audit_is_strictly_clean() {
    assert_eq!(LIVE_WORKSPACE_RUNFILES, "required");
    xtask::rmat_audit::rmat_audit(xtask::rmat_audit::RmatAuditArgs {
        strict: true,
        json: false,
        update_baseline: false,
    })
    .expect("strict RMAT audit should be clean for the committed workspace");
}
