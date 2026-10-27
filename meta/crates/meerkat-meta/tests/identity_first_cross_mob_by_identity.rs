//! `wire_cross_mob_by_identity` across two in-process mobs whose identity
//! runtimes are installed in their `UnifiedRuntime`s.
//!
//! A generated alias (`rt:{identity}:{generation}`) resolves only under the
//! identity runtime that owns the identity: the local side's installed
//! runtime, and the authority the peer mob was registered with. The helper
//! takes the runtimes' exported views, so a host never needs the inner `Arc`.

#![cfg(not(target_arch = "wasm32"))]
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use meerkat_client::TestClient;
use meerkat_meta::contact_directory::ContactDirectory;
use meerkat_meta::identity_first::contracts::RosterProvider;
use meerkat_meta::identity_first::{
    AgentAddressability, AgentIdentity, DurableAgentSpec, RosterContext, RosterError,
    wire_cross_mob_by_identity,
};
use meerkat_meta::{UnifiedRuntime, UnifiedRuntimeBuilder};
use meerkat_mob::{MobDefinition, ProfileName};

fn id(name: &str) -> AgentIdentity {
    AgentIdentity::parse(name).expect("identity")
}

fn definition(mob_id: &str) -> MobDefinition {
    MobDefinition::from_toml(&format!(
        r#"
[mob]
id = "{mob_id}"

[profiles.worker]
model = "gpt-5.5"
external_addressable = true

[profiles.worker.tools]
comms = true
"#
    ))
    .expect("mob definition")
}

/// One addressable worker identity.
struct OneWorker(&'static str);

#[async_trait::async_trait]
impl RosterProvider for OneWorker {
    async fn roster(&self, _context: &RosterContext) -> Result<Vec<DurableAgentSpec>, RosterError> {
        Ok(vec![DurableAgentSpec {
            identity: id(self.0),
            profile: ProfileName::from("worker"),
            addressability: AgentAddressability::Addressable,
            display_name: None,
            labels: BTreeMap::new(),
            context: None,
            additional_instructions: Vec::new(),
            initial_message: None,
            runtime_mode_override: None,
            backend: None,
            binding: None,
            placement: None,
        }])
    }
}

/// An identity-first runtime: the builder installs its identity runtime and
/// creates the roster's identity at build.
async fn identity_first(
    mob_id: &str,
    identity: &'static str,
    state: &std::path::Path,
    directory: ContactDirectory,
) -> UnifiedRuntime {
    Box::pin(
        UnifiedRuntimeBuilder::default()
            .definition(definition(mob_id))
            .persistent_state(state)
            .continuity_from_state_dir(state)
            .await
            .expect("open the state-dir identity substrate")
            .roster_provider(Arc::new(OneWorker(identity)))
            .identity_runtime_instance_id(mob_id)
            .comms(true)
            .contact_directory(directory)
            .default_llm_client(Arc::new(TestClient::default()))
            .build(),
    )
    .await
    .expect("build the identity-first runtime")
}

#[tokio::test(flavor = "multi_thread")]
async fn the_cross_mob_helper_wires_identities_of_installed_identity_runtimes() {
    // In-process peer routes must not collide with concurrently running tests.
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let left_mob = format!("helper-left-{}-{n}", std::process::id());
    let right_mob = format!("helper-right-{}-{n}", std::process::id());
    let directory = ContactDirectory::from_toml(&format!(
        "[mobs]\n\"{left_mob}\" = \"inproc\"\n\"{right_mob}\" = \"inproc\""
    ))
    .expect("contact directory");
    let temp = tempfile::TempDir::new().expect("temp dir");
    let left = identity_first(
        &left_mob,
        "left:alice",
        &temp.path().join("left"),
        directory.clone(),
    )
    .await;
    let right = identity_first(
        &right_mob,
        "right:bob",
        &temp.path().join("right"),
        directory,
    )
    .await;
    // Each side registers the other with its identity runtime, the authority
    // for the peer's generated aliases.
    left.register_peer_runtime(&right).await;
    right.register_peer_runtime(&left).await;

    let left_identities = left.identity_runtime().expect("left identity runtime");
    let right_identities = right.identity_runtime().expect("right identity runtime");
    let alice = left_identities
        .runtime_id_for(&id("left:alice"))
        .await
        .expect("alice's runtime id");
    assert!(
        alice.as_str().starts_with("rt:"),
        "the helper wires a generated alias: {alice}"
    );

    wire_cross_mob_by_identity(
        &left_identities,
        &id("left:alice"),
        &right_identities,
        &id("right:bob"),
        &left,
        &right_mob,
    )
    .await
    .expect("the helper wires identities of installed identity runtimes");

    drop(left_identities);
    drop(right_identities);
    left.shutdown().await;
    right.shutdown().await;
}
