#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use meerkat::{ConfigDelta, SdkConfigStore};
use serde_json::json;

#[test]
fn e2e_sdk_config_ephemeral() {
    let store = SdkConfigStore::new();
    let baseline = store.get().expect("get config");

    let patched = store
        .patch(ConfigDelta(json!({
            "agent": { "model": "ephemeral-model" }
        })))
        .expect("patch config");

    assert_eq!(patched.agent.model, "ephemeral-model");

    let fresh_store = SdkConfigStore::new();
    let fresh = fresh_store.get().expect("get config");

    assert_eq!(
        fresh.agent.model, baseline.agent.model,
        "ephemeral override should not persist across stores"
    );
}
