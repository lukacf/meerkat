//! The `meta/shutdown` result of a stock persistent gateway.
//!
//! A gateway over a persistent root keeps agent-created child mobs durable by
//! default. Its normal shutdown stands them and the agent mob-tool state down
//! and releases their root, so cleanup is complete and the response carries
//! no diagnostics. This checks that exact shape, not a weaker one: a wedged
//! drain, a failed mob stop, an unreleased identity authority, an orphaned
//! module process, an unfinished retired cleanup or a held child root still
//! fails it.
#![allow(clippy::panic, dead_code)]

use serde_json::Value;

pub fn assert_persistent_shutdown_completed(response: &Value, context: &str) {
    let result = &response["result"];
    assert_eq!(result["shutdown"], true, "{context}: {response}");
    assert_eq!(
        result["runtime_cleanup_completed"], true,
        "{context}: {response}"
    );
    assert!(
        result.get("runtime_cleanup_retained_until_exit").is_none(),
        "{context}: a released child root is not process-exit retention: {response}"
    );
    assert!(
        result.get("runtime_cleanup_diagnostics").is_none(),
        "{context}: a completed cleanup carries no diagnostics: {response}"
    );
}
