#![allow(clippy::expect_used)]

const LIVE_WORKSPACE_RUNFILES: &str = "required";

#[test]
fn live_workspace_seam_inventory_is_strictly_clean() {
    assert_eq!(LIVE_WORKSPACE_RUNFILES, "required");
    xtask::seam_inventory::run_seam_inventory(xtask::seam_inventory::SeamInventoryArgs {
        strict: true,
    })
    .expect("strict seam inventory should be clean for the committed workspace");
}
