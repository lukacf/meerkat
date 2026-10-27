//! D50 S2, Meta side: the host's native member registration hook reaches
//! every mob the spec builds (the child-mob path is pinned in
//! `child_application_tool_policy.rs`, which drives a real delegate). Root-mob members and child-mob members (agent
//! `mob_create`, delegate's implicit mob) select their destination through
//! the same hook, whichever order the host configures the hook and the agent
//! mob tools in. A builder and a supplied spec cannot both set one, and a
//! hook without the canonical runtime adapter is refused, never skipped.
//!
//! The hook contract itself (cold install on the exact registration witness,
//! attached verification, rollback) is Core's and is tested there; these
//! tests pin only the Meta threading.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use meerkat_core::auth::{PrincipalKind, PrincipalRef};
use meerkat_meta::{MobBootstrapOptions, MobBootstrapSpec, MobRuntime, UnifiedRuntime};
use meerkat_mob::{
    MemberDestinationSelection, MemberRuntimeBuildIdentity, MemberRuntimeRegistrationHook, MobError,
};

static NEXT_MOB: AtomicUsize = AtomicUsize::new(0);

/// Records every member it is asked about and selects the member's default
/// destination.
#[derive(Default)]
struct RecordingHook {
    selected: Mutex<Vec<String>>,
}

impl RecordingHook {
    fn selected(&self) -> Vec<String> {
        self.selected.lock().unwrap().clone()
    }
}

#[async_trait::async_trait]
impl MemberRuntimeRegistrationHook for RecordingHook {
    async fn select_destination(
        &self,
        identity: &MemberRuntimeBuildIdentity,
    ) -> Result<MemberDestinationSelection, MobError> {
        self.selected
            .lock()
            .unwrap()
            .push(identity.member().member.clone());
        Ok(selection(identity))
    }

    async fn expected_destination(
        &self,
        identity: &MemberRuntimeBuildIdentity,
    ) -> Result<MemberDestinationSelection, MobError> {
        Ok(selection(identity))
    }

    /// No installed-provider claims: any surface is acceptable to this host.
    async fn verify_member_surface(
        &self,
        _identity: &MemberRuntimeBuildIdentity,
        _surface: &[meerkat_core::ToolDef],
    ) -> Result<(), MobError> {
        Ok(())
    }
}

/// A destination owned by the member itself, in a context named after it.
/// The context and audience are built through their wire form, so this
/// test names no type from the unpublished authorization-contracts crate.
fn selection(identity: &MemberRuntimeBuildIdentity) -> MemberDestinationSelection {
    let owner = PrincipalRef::new(PrincipalKind::SharedAgent, identity.member().member.clone())
        .expect("member principal");
    MemberDestinationSelection {
        logical_owner: owner.clone(),
        context: identity.session_document().clone(),
        context_generation: 1,
        audience: meerkat_mob::AudienceRef::Principal { principal: owner },
    }
}

fn definition() -> meerkat_mob::MobDefinition {
    meerkat_mob::MobDefinition::from_toml(&format!(
        r#"
[mob]
id = "registration-hook-{}-{}"

[profiles.worker]
model = "gpt-5.5"

[profiles.worker.tools]
comms = true
"#,
        std::process::id(),
        NEXT_MOB.fetch_add(1, Ordering::SeqCst)
    ))
    .expect("mob definition")
}

fn options() -> MobBootstrapOptions {
    MobBootstrapOptions {
        allow_ephemeral_sessions: true,
        notify_orchestrator_on_resume: true,
        default_llm_client: Some(Arc::new(meerkat_client::TestClient::default())),
    }
}

/// A spec whose session service is backed by the canonical runtime adapter,
/// which a registration hook requires.
async fn persistent_spec(dir: &tempfile::TempDir) -> MobBootstrapSpec {
    let (storage, provenance) = meerkat_meta::mob_composition_manifest::persistent_mob_storage(
        dir.path().join("mob.sqlite3"),
    )
    .expect("open persistent mob storage");
    MobBootstrapSpec::persistent(
        definition(),
        storage,
        provenance,
        dir.path().to_path_buf(),
        4,
    )
    .await
    .expect("compose persistent stores")
    .with_options(options())
}

/// A spec built WITHOUT the host's grant owner cannot install a hooked
/// member's destination (S3 requires the governed work host): the member is
/// refused at registration, never registered unscoped. Hooked members are
/// built through `UnifiedRuntimeBuilder::native_grant_work_configuration`
/// (see `native_grant_configuration.rs`).
#[tokio::test(flavor = "multi_thread")]
async fn a_hooked_member_without_the_host_grant_owner_is_refused_not_skipped() {
    for persistent in [true, false] {
        let dir = tempfile::tempdir().unwrap();
        let hook = Arc::new(RecordingHook::default());
        let spec = if persistent {
            persistent_spec(&dir).await
        } else {
            MobBootstrapSpec::ephemeral(
                definition(),
                meerkat_mob::MobStorage::in_memory(),
                dir.path().to_path_buf(),
                4,
                None,
            )
            .expect("ephemeral spec")
            .with_options(options())
        };
        let runtime =
            MobRuntime::bootstrap(spec.with_member_runtime_registration_hook(hook.clone()))
                .await
                .expect("bootstrap");
        let error = runtime
            .handle()
            .ensure_member(meerkat_mob::SpawnMemberSpec::new(
                meerkat_mob::ProfileName::from("worker"),
                meerkat_mob::ids::AgentIdentity::from("worker-1"),
            ))
            .await
            .expect_err("no governed work host: the hooked member is refused");
        assert!(
            error.to_string().contains("governed work host"),
            "persistent={persistent}: {error}"
        );
        // The hook WAS consulted: the refusal is the install, not a skip.
        assert_eq!(hook.selected().len(), 1, "persistent={persistent}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_hook_on_both_the_builder_and_the_supplied_spec_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let error = UnifiedRuntime::builder()
        .mob_spec(
            persistent_spec(&dir)
                .await
                .with_member_runtime_registration_hook(Arc::new(RecordingHook::default())),
        )
        .member_runtime_registration_hook(Arc::new(RecordingHook::default()))
        .module_config(meerkat_meta::MetaConfig {
            modules: vec![],
            discovery: meerkat_meta::DiscoverySpec {
                namespace: "registration-hook".to_string(),
                modules: vec![],
            },
            pre_spawn: vec![],
        })
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .await
        .err()
        .expect("a doubly configured hook is refused");
    assert!(
        error.to_string().contains("member registration hook"),
        "{error}"
    );
}
