//! No member may take a console runtime-plane identity (`_system` or
//! `__console__`). The console treats frames under those identities as the
//! runtime's own events, so every path that creates a member refuses them:
//! agent tools spawning into the host mob or a child mob, same-mob forks,
//! spawns whose identity a host customizer rewrites, the members a runtime's
//! bootstrap discovery declares, and sends to a name the definition's spawn
//! policy maps. The guard that refuses them keeps asking a host customizer
//! about every member a restart restores.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use meerkat_core::service::MobToolAuthorityContext;
use meerkat_core::types::{SessionId, ToolCallView};
use meerkat_core::{AgentToolDispatcher, ToolDispatchOutcome, ToolError};
use meerkat_meta::{MobBootstrapOptions, MobBootstrapSpec, MobRuntime};
use meerkat_mob_mcp::MobMcpState;
use serde_json::{Value, json};

static NEXT_MOB: AtomicUsize = AtomicUsize::new(0);

fn definition() -> meerkat_mob::MobDefinition {
    meerkat_mob::MobDefinition::from_toml(&format!(
        r#"
[mob]
id = "runtime-plane-reservation-{}-{}"

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

async fn boot(
    dir: &tempfile::TempDir,
    configure: impl FnOnce(MobBootstrapSpec) -> MobBootstrapSpec,
) -> MobRuntime {
    let spec = MobBootstrapSpec::ephemeral(
        definition(),
        meerkat_mob::MobStorage::in_memory(),
        dir.path().to_path_buf(),
        4,
        None,
    )
    .expect("build the ephemeral spec")
    .with_options(options());
    MobRuntime::bootstrap(configure(spec))
        .await
        .expect("bootstrap the mob runtime")
}

/// Durable sessions and mob storage: `fork_off` needs the session service's
/// durable transcript fork authority.
async fn boot_persistent(dir: &tempfile::TempDir) -> MobRuntime {
    boot_persistent_with(dir, definition(), None).await
}

/// One durable lifetime over `dir`; a second call over the same directory
/// restores what the first one left.
async fn boot_persistent_with(
    dir: &tempfile::TempDir,
    definition: meerkat_mob::MobDefinition,
    customizer: Option<Arc<dyn meerkat_mob::SpawnMemberCustomizer>>,
) -> MobRuntime {
    let (storage, provenance) = meerkat_meta::mob_composition_manifest::persistent_mob_storage(
        dir.path().join("mob.sqlite3"),
    )
    .expect("open persistent mob storage");
    let mut spec =
        MobBootstrapSpec::persistent(definition, storage, provenance, dir.path().to_path_buf(), 4)
            .await
            .expect("compose persistent stores")
            .with_options(options());
    if let Some(customizer) = customizer {
        spec = spec.with_spawn_member_customizer(customizer);
    }
    MobRuntime::bootstrap(spec)
        .await
        .expect("bootstrap the mob runtime")
}

fn creator_authority() -> MobToolAuthorityContext {
    meerkat_runtime::mob_operator_authority::create_only_mob_operator_authority()
        .expect("generated authority")
}

fn manage_authority(runtime: &MobRuntime) -> MobToolAuthorityContext {
    let handle = runtime.handle();
    meerkat_runtime::mob_operator_authority::grant_manage_mob(
        &creator_authority(),
        handle.mob_id().as_str(),
        handle.live_incarnation(),
    )
    .expect("manage scope")
}

fn spawn_worker_authority(runtime: &MobRuntime) -> MobToolAuthorityContext {
    let handle = runtime.handle();
    meerkat_runtime::mob_operator_authority::grant_spawn_profile_in_mob(
        &meerkat_runtime::mob_operator_authority::set_create_authority(&creator_authority(), false)
            .expect("no create scope"),
        handle.mob_id().as_str(),
        handle.live_incarnation(),
        "worker",
    )
    .expect("spawnable worker profile")
}

fn agent_surface(
    state: Arc<MobMcpState>,
    session: SessionId,
    authority: MobToolAuthorityContext,
) -> Arc<dyn AgentToolDispatcher> {
    let surface: Arc<dyn AgentToolDispatcher> =
        Arc::new(meerkat_mob_mcp::AgentMobToolSurface::new(
            state,
            None,
            authority,
            "gpt-5.5".to_string(),
            session.clone(),
            None,
            None,
            None,
        ));
    surface
        .bind_ops_lifecycle(
            Arc::new(meerkat_runtime::ops_lifecycle::RuntimeOpsLifecycleRegistry::new()),
            session,
        )
        .expect("bind ops lifecycle")
        .into_dispatcher()
}

async fn dispatch(
    surface: &Arc<dyn AgentToolDispatcher>,
    name: &'static str,
    args: Value,
) -> Result<ToolDispatchOutcome, ToolError> {
    let raw = serde_json::value::RawValue::from_string(args.to_string()).unwrap();
    tokio::time::timeout(
        std::time::Duration::from_mins(1),
        surface.dispatch(ToolCallView {
            id: "reservation-call",
            name,
            args: &raw,
        }),
    )
    .await
    .unwrap_or_else(|_| panic!("{name} returns within the failure bound"))
}

/// The call is refused for the reservation, not for some other reason.
fn refused_as_reserved(outcome: &Result<ToolDispatchOutcome, ToolError>) -> bool {
    match outcome {
        Err(error) => error.to_string().contains("reserved"),
        Ok(outcome) => {
            outcome.result.is_error && outcome.result.text_content().contains("reserved")
        }
    }
}

fn describe(outcome: &Result<ToolDispatchOutcome, ToolError>) -> String {
    match outcome {
        Err(error) => format!("refused: {error}"),
        Ok(outcome) => format!(
            "answered (is_error {}): {}",
            outcome.result.is_error,
            outcome.result.text_content()
        ),
    }
}

fn agent_mob_tools(runtime: &MobRuntime) -> Arc<MobMcpState> {
    runtime
        .agent_mob_mcp_state()
        .expect("the agent mob tools are installed")
}

async fn seat(runtime: &MobRuntime, member: &str) -> SessionId {
    let handle = runtime.handle();
    handle
        .ensure_member(meerkat_mob::SpawnMemberSpec::new(
            meerkat_mob::ProfileName::from("worker"),
            meerkat_mob::AgentIdentity::from(member),
        ))
        .await
        .expect("seat member");
    handle
        .resolve_bridge_session_id(&meerkat_mob::AgentIdentity::from(member))
        .await
        .expect("member session")
}

async fn roster_has(runtime: &MobRuntime, member: &str) -> bool {
    runtime
        .handle()
        .roster()
        .await
        .get_by_identity(&meerkat_mob::AgentIdentity::from(member))
        .is_some()
}

/// An agent's `mob_spawn_member` into the host mob refuses both
/// runtime-plane identities, including the encoded roster spelling of
/// `_system` that console attribution decodes back to it.
#[tokio::test(flavor = "multi_thread")]
async fn agent_tool_spawn_into_the_host_mob_refuses_runtime_plane_identities() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = boot(&dir, |spec| spec).await;
    let session = seat(&runtime, "lead").await;
    let mob_id = runtime.handle().mob_id().to_string();
    let surface = agent_surface(
        agent_mob_tools(&runtime),
        session,
        manage_authority(&runtime),
    );
    for member_id in ["__console__", "mk--__system"] {
        let outcome = dispatch(
            &surface,
            "mob_spawn_member",
            json!({
                "mob_id": mob_id,
                "profile": "worker",
                "member_id": member_id,
                "runtime_mode": "turn_driven",
            }),
        )
        .await;
        assert!(
            refused_as_reserved(&outcome),
            "{member_id} is refused: {}",
            describe(&outcome)
        );
        assert!(
            !roster_has(&runtime, member_id).await,
            "no {member_id} member"
        );
    }
}

/// A mob an agent creates with `mob_create` is built by the agent mob tools;
/// a `mob_spawn_member` into it refuses a runtime-plane identity.
#[tokio::test(flavor = "multi_thread")]
async fn agent_tool_spawn_into_a_child_mob_refuses_runtime_plane_identities() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = boot(&dir, |spec| spec).await;
    let session = seat(&runtime, "lead").await;
    let state = agent_mob_tools(&runtime);
    let surface = agent_surface(Arc::clone(&state), session.clone(), creator_authority());
    let child_mob_id = format!("child-{}", NEXT_MOB.fetch_add(1, Ordering::SeqCst));
    let created = dispatch(
        &surface,
        "mob_create",
        json!({
            "definition": {
                "id": child_mob_id,
                "profiles": { "worker": { "model": "gpt-5.5", "tools": { "comms": true } } }
            }
        }),
    )
    .await;
    assert!(
        matches!(&created, Ok(outcome) if !outcome.result.is_error),
        "the child mob is created: {}",
        describe(&created)
    );
    // The turn owner commits the create's operator grant; the agent's next
    // call runs under it.
    let granted = created
        .expect("created")
        .session_effects
        .into_iter()
        .find_map(|effect| match effect {
            meerkat_core::SessionEffect::ReplaceMobToolAuthorityContext { authority_context } => {
                Some(authority_context)
            }
            _ => None,
        })
        .expect("the create grants its creator the new mob");
    let surface = agent_surface(state, session, granted);
    let outcome = dispatch(
        &surface,
        "mob_spawn_member",
        json!({
            "mob_id": child_mob_id,
            "profile": "worker",
            "member_id": "_system",
            "runtime_mode": "turn_driven",
        }),
    )
    .await;
    assert!(
        refused_as_reserved(&outcome),
        "a `_system` member of the child mob is refused: {}",
        describe(&outcome)
    );
}

/// A same-mob `fork_off` refuses a runtime-plane child identity.
#[tokio::test(flavor = "multi_thread")]
async fn fork_off_refuses_runtime_plane_identities() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = boot_persistent(&dir).await;
    let session = seat(&runtime, "alice").await;
    let forker = agent_surface(
        agent_mob_tools(&runtime),
        session,
        spawn_worker_authority(&runtime),
    );
    let outcome = dispatch(
        &forker,
        "fork_off",
        json!({"member_id": "__console__", "task": "check the status board"}),
    )
    .await;
    assert!(
        refused_as_reserved(&outcome),
        "a `__console__` fork is refused: {}",
        describe(&outcome)
    );
    assert!(!roster_has(&runtime, "__console__").await, "no fork member");
}

/// Renames the spawn the way a host customizer may.
struct RenameSpawn(&'static str);

impl meerkat_mob::SpawnMemberCustomizer for RenameSpawn {
    fn customize_spawn(
        &self,
        _ctx: &meerkat_mob::SpawnCustomizationContext,
        spec: &mut meerkat_mob::SpawnMemberSpec,
    ) -> Result<(), meerkat_mob::MobError> {
        spec.identity = meerkat_mob::AgentIdentity::from(self.0);
        Ok(())
    }
}

/// Sets the console identity override label the way a host customizer may.
struct LabelSpawn(&'static str);

impl meerkat_mob::SpawnMemberCustomizer for LabelSpawn {
    fn customize_spawn(
        &self,
        _ctx: &meerkat_mob::SpawnCustomizationContext,
        spec: &mut meerkat_mob::SpawnMemberSpec,
    ) -> Result<(), meerkat_mob::MobError> {
        spec.labels
            .get_or_insert_with(BTreeMap::new)
            .insert("agent_identity".to_string(), self.0.to_string());
        Ok(())
    }
}

/// The check sees the spawn as the host's customizers leave it: a customizer
/// that renames a fresh spawn onto a runtime-plane identity cannot land it.
#[tokio::test(flavor = "multi_thread")]
async fn a_host_customizer_cannot_rename_a_spawn_onto_a_runtime_plane_identity() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = boot(&dir, |spec| {
        spec.with_spawn_member_customizer(Arc::new(RenameSpawn("_system")))
    })
    .await;
    let spawned = runtime
        .handle()
        .spawn_spec(meerkat_mob::SpawnMemberSpec::new(
            meerkat_mob::ProfileName::from("worker"),
            meerkat_mob::AgentIdentity::from("worker-renamed"),
        ))
        .await;
    match &spawned {
        Err(error) => assert!(
            error.to_string().contains("reserved"),
            "the renamed spawn is refused for the reservation: {error}"
        ),
        Ok(_) => panic!("a spawn renamed to `_system` is refused"),
    }
    assert!(
        !roster_has(&runtime, "_system").await,
        "no `_system` member"
    );
    assert!(
        !roster_has(&runtime, "worker-renamed").await,
        "no member under the requested name either"
    );
}

/// A customizer that labels a fresh spawn with a runtime-plane console
/// identity override cannot land it either.
#[tokio::test(flavor = "multi_thread")]
async fn a_host_customizer_cannot_label_a_spawn_with_a_runtime_plane_identity() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = boot(&dir, |spec| {
        spec.with_spawn_member_customizer(Arc::new(LabelSpawn("__console__")))
    })
    .await;
    let spawned = runtime
        .handle()
        .spawn_spec(meerkat_mob::SpawnMemberSpec::new(
            meerkat_mob::ProfileName::from("worker"),
            meerkat_mob::AgentIdentity::from("worker-labelled"),
        ))
        .await;
    match &spawned {
        Err(error) => assert!(
            error.to_string().contains("reserved"),
            "the labelled spawn is refused for the reservation: {error}"
        ),
        Ok(_) => panic!("a spawn labelled with the `__console__` identity is refused"),
    }
    assert!(
        !roster_has(&runtime, "worker-labelled").await,
        "no member under the override"
    );
}

/// Records every member a restart rebuilds on its durable session.
#[derive(Default)]
struct ResumeRecorder(std::sync::Mutex<Vec<(String, SessionId)>>);

impl meerkat_mob::SpawnMemberCustomizer for ResumeRecorder {
    fn customize_spawn(
        &self,
        _ctx: &meerkat_mob::SpawnCustomizationContext,
        _spec: &mut meerkat_mob::SpawnMemberSpec,
    ) -> Result<(), meerkat_mob::MobError> {
        Ok(())
    }

    fn customize_resume(
        &self,
        _ctx: &meerkat_mob::SpawnCustomizationContext,
        durable: &meerkat_mob::ResumedMemberView,
        spec: &mut meerkat_mob::SpawnMemberSpec,
    ) -> Result<(), meerkat_mob::MobError> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push((spec.identity.to_string(), durable.session_id.clone()));
        Ok(())
    }
}

/// The guard composes over a host customizer without hiding its resume ask:
/// a restart still hands the host customizer each restored member with the
/// session it is rebuilt on.
#[tokio::test(flavor = "multi_thread")]
async fn a_host_customizer_still_sees_every_restored_member_resume() {
    let dir = tempfile::tempdir().unwrap();
    let definition = definition();
    let first = boot_persistent_with(&dir, definition.clone(), None).await;
    let session = seat(&first, "alice").await;
    first
        .handle()
        .shutdown()
        .await
        .expect("first lifetime shuts down");
    first.stand_down_child_mobs(stand_down_deadline()).await;
    drop(first);

    let recorder = Arc::new(ResumeRecorder::default());
    let second = boot_persistent_with(
        &dir,
        definition,
        Some(Arc::clone(&recorder) as Arc<dyn meerkat_mob::SpawnMemberCustomizer>),
    )
    .await;
    assert!(roster_has(&second, "alice").await, "alice is restored");
    let resumed = recorder
        .0
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    assert!(
        resumed.contains(&("alice".to_string(), session)),
        "the host customizer is asked to resume alice on her session: {resumed:?}"
    );
    second
        .handle()
        .shutdown()
        .await
        .expect("second lifetime shuts down");
    second.stand_down_child_mobs(stand_down_deadline()).await;
}

/// The members a runtime's bootstrap discovery declares.
struct DeclaredMembers(Vec<meerkat_meta::AgentDiscoverySpec>);

impl meerkat_meta::Discovery for DeclaredMembers {
    fn discover(
        &self,
        _context: Value,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Vec<meerkat_meta::AgentDiscoverySpec>> + Send + '_>,
    > {
        let specs = self.0.clone();
        Box::pin(async move { specs })
    }
}

/// Builds a runtime whose bootstrap discovery declares `member_ids`; the
/// error is the build's own.
async fn bootstrap_with_declared_members(
    dir: &tempfile::TempDir,
    member_ids: &[&str],
) -> Result<meerkat_meta::UnifiedRuntime, String> {
    let spec = MobBootstrapSpec::ephemeral(
        definition(),
        meerkat_mob::MobStorage::in_memory(),
        dir.path().to_path_buf(),
        4,
        None,
    )
    .expect("build the ephemeral spec")
    .with_options(options());
    let declared = member_ids
        .iter()
        .map(|member_id| meerkat_meta::AgentDiscoverySpec {
            profile: "worker".to_string(),
            meerkat_id: (*member_id).to_string(),
            labels: None,
            context: None,
            additional_instructions: Vec::new(),
            resume_session_id: None,
        })
        .collect();
    Box::pin(
        meerkat_meta::UnifiedRuntime::builder()
            .mob_spec(spec)
            .module_config(meerkat_meta::MetaConfig {
                modules: Vec::new(),
                discovery: meerkat_meta::DiscoverySpec {
                    namespace: "runtime-plane-reservation".to_string(),
                    modules: Vec::new(),
                },
                pre_spawn: Vec::new(),
            })
            .timeout(std::time::Duration::from_secs(2))
            .discovery(DeclaredMembers(declared))
            .build(),
    )
    .await
    .map_err(|error| error.to_string())
}

/// A member the bootstrap discovery declares under a runtime-plane identity
/// stops the build; an ordinary declared member still comes up.
#[tokio::test(flavor = "multi_thread")]
async fn a_bootstrap_member_cannot_take_a_runtime_plane_identity() {
    for member_id in ["__console__", "_system"] {
        let dir = tempfile::tempdir().unwrap();
        match bootstrap_with_declared_members(&dir, &["declared-ok", member_id]).await {
            Err(error) => assert!(
                error.contains("reserved"),
                "{member_id}: the build names the reservation: {error}"
            ),
            Ok(runtime) => {
                let roster = runtime
                    .mob_handle()
                    .list_members()
                    .await
                    .iter()
                    .map(|member| member.agent_identity.to_string())
                    .collect::<Vec<_>>();
                runtime.shutdown().await;
                panic!(
                    "a bootstrap member named {member_id} is refused; the roster holds {roster:?}"
                );
            }
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let runtime = bootstrap_with_declared_members(&dir, &["declared-ok"])
        .await
        .expect("an ordinary declared member comes up");
    let declared = runtime
        .mob_handle()
        .list_members()
        .await
        .iter()
        .any(|member| member.agent_identity.as_str() == "declared-ok");
    runtime.shutdown().await;
    assert!(declared, "the declared member is on the roster");
}

fn stand_down_deadline() -> std::time::Instant {
    std::time::Instant::now() + std::time::Duration::from_secs(90)
}

/// Core auto-spawns a member the definition's spawn policy maps only for
/// external work a host submits through `MobHandle::submit_work` to a member
/// the roster does not hold. Meta's sends resolve the member first, so a send
/// to a runtime-plane identity the policy maps creates no member.
#[tokio::test(flavor = "multi_thread")]
async fn a_send_creates_no_runtime_plane_member_the_spawn_policy_maps() {
    let dir = tempfile::tempdir().unwrap();
    let definition = meerkat_mob::MobDefinition::from_toml(&format!(
        r#"
[mob]
id = "runtime-plane-policy-{}-{}"

[profiles.worker]
model = "gpt-5.5"
external_addressable = true

[profiles.worker.tools]
comms = true

[spawn_policy]
mode = "auto"

[spawn_policy.profile_map]
__console__ = "worker"
_system = "worker"
policy-ok = "worker"
"#,
        std::process::id(),
        NEXT_MOB.fetch_add(1, Ordering::SeqCst)
    ))
    .expect("mob definition");
    let spec = MobBootstrapSpec::ephemeral(
        definition,
        meerkat_mob::MobStorage::in_memory(),
        dir.path().to_path_buf(),
        4,
        None,
    )
    .expect("build the ephemeral spec")
    .with_options(options());
    let runtime = MobRuntime::bootstrap(spec)
        .await
        .expect("bootstrap the mob runtime");
    let handle = runtime.handle();
    for member_id in ["__console__", "_system"] {
        let sent = meerkat_meta::send_message_on_mob(&handle, member_id, "hello").await;
        assert!(sent.is_err(), "a send to {member_id} is not delivered");
        assert!(
            !roster_has(&runtime, member_id).await,
            "no {member_id} member"
        );
    }
    // The policy is live: a host's external work for an ordinary name it
    // maps spawns that member.
    handle
        .submit_work(
            meerkat_mob::AgentRuntimeId::initial(meerkat_mob::AgentIdentity::from("policy-ok")),
            meerkat_mob::FenceToken::new(0),
            meerkat_mob::WorkRef::new(),
            meerkat_mob::WorkSpec::new("hello".to_string(), meerkat_mob::WorkOrigin::External),
        )
        .await
        .expect("the policy spawns an ordinary name a host submits work to");
    assert!(
        roster_has(&runtime, "policy-ok").await,
        "the policy spawned the ordinary member"
    );
}
