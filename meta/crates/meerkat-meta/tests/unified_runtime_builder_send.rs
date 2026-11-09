#![allow(clippy::expect_used)]
//! `UnifiedRuntimeBuilder::build()` returns a `Send` future, so a host can
//! `tokio::spawn(builder.build())` or drive it under a harness that requires
//! `Send`. The check is structural (auto traits follow field and await types),
//! so these fail to compile if a non-`Sync` builder field comes back while
//! `build` holds `&self` across an await, or if anything `build` awaits holds
//! a borrow in a way the compiler cannot prove `Send` for every lifetime.

use meerkat_meta::{UnifiedRuntime, UnifiedRuntimeBuilder};

fn assert_send<T: Send>(_: &T) {}

#[test]
fn the_builder_is_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<UnifiedRuntimeBuilder>();
}

#[test]
fn the_build_future_is_send() {
    let build = UnifiedRuntime::builder().build();
    assert_send(&build);
}

/// The host-facing shape: a builder carrying a pre-spawn hook (the field that
/// made the future `!Send`) is spawned onto the runtime. With no definition
/// the build is refused, typed; what matters is that it compiles and runs.
#[tokio::test]
async fn a_build_with_a_pre_spawn_hook_can_be_spawned() {
    let builder = UnifiedRuntime::builder()
        .pre_spawn_hook(Box::new(|| Box::pin(async { Ok(serde_json::Value::Null) })));
    let build = tokio::spawn(builder.build());
    let outcome = build.await.expect("the spawned build task completes");
    assert!(outcome.is_err(), "a build with no definition is refused");
}
