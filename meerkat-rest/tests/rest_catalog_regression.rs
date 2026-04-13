#![allow(clippy::expect_used)]

use meerkat_contracts::rest_documented_paths;

#[test]
fn documented_rest_surface_keeps_live_config_and_member_routes() {
    let paths = rest_documented_paths();
    for expected in [
        "/config",
        "/mob/{id}/members/{agent_identity}/status",
        "/mob/{id}/members/{agent_identity}/cancel",
        "/mob/{id}/members/{agent_identity}/respawn",
    ] {
        assert!(
            paths.iter().any(|path| path == &expected),
            "documented REST surface dropped live route {expected}"
        );
    }
}
