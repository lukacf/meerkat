#![allow(clippy::panic)]
//! E2E check for macOS system proxy discovery.
//!
//! This test intentionally uses the default reqwest client builder to exercise
//! system proxy discovery. It should be run only in full-access environments.

#[test]
#[ignore = "E2E: requires full macOS system proxy access"]
fn test_system_proxy_discovery_does_not_panic() {
    let result = std::panic::catch_unwind(|| {
        let _client = reqwest::Client::builder()
            .build()
            .expect("reqwest client build failed");
    });

    if result.is_err() {
        panic!(
            "System proxy discovery panicked. This usually indicates a restricted \
             or headless macOS environment. Run this test in a full-access environment."
        );
    }
}
