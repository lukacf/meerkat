//! D50 S1 / GAP-E1: an embedder's actual native grant owner reaches the
//! runtime machine Meta constructs. `UnifiedRuntimeBuilder::
//! native_grant_work_configuration` installs exactly the supplied owner and
//! its paired invocation policy (Meta issues nothing). A pre-built spec cannot
//! take one, and a supplied owner is never silently dropped. The composed
//! gateway takes the owner from its host before it builds the machine, and a
//! restarted host installs its restored owner.
//!
//! The configuration here refuses everything: these tests pin the Meta
//! threading, not grant semantics (Core's native governed loop owns those).
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::sync::Arc;

use meerkat_authorization::clock::HostAuthorizationClock;
use meerkat_authorization::grant_policy::{
    AdmittedWorkPolicyOwner, OperationPolicyOwner, WorkOwnerAllowance,
};
use meerkat_authorization::grants::{
    FileGrantOwnerStore, GrantDurabilityError, GrantOwnerStore, LocalGrantAuthority,
    LocalGrantConfiguration,
};
use meerkat_authorization::policy::{LocalPolicyAllowance, LocalPolicyPurpose};
use meerkat_authorization::publication::LocalAuthorizationPublication;
use meerkat_authorization_contracts::constraints::ExecutionRestrictions;
use meerkat_authorization_contracts::evidence::EvidenceId;
use meerkat_authorization_contracts::grant::GrantLineageRef;
use meerkat_authorization_contracts::work_association::InputAuthorityAssociation;
use meerkat_core::auth::{PrincipalKind, PrincipalRef};
use meerkat_core::{OperationAuthorizationError, PreparedAuthorizationBinding};
use meerkat_meta::UnifiedRuntime;
use meerkat_runtime::input_authority::NativeAdmissionError;
use meerkat_runtime::meerkat_machine::NativeGrantWorkConfiguration;

struct RefusingOwner;

impl AdmittedWorkPolicyOwner for RefusingOwner {
    fn authorize_admitted_work(
        &self,
        _association: &InputAuthorityAssociation,
        _binding: &PreparedAuthorizationBinding,
        _purpose: LocalPolicyPurpose,
        _now_ms: u64,
    ) -> Result<WorkOwnerAllowance, OperationAuthorizationError> {
        Err(OperationAuthorizationError::Unavailable)
    }
}

impl OperationPolicyOwner for RefusingOwner {
    fn authorize_operation(
        &self,
        _association: &InputAuthorityAssociation,
        _binding: &PreparedAuthorizationBinding,
        _purpose: LocalPolicyPurpose,
        _now_ms: u64,
    ) -> Result<LocalPolicyAllowance, OperationAuthorizationError> {
        Err(OperationAuthorizationError::Unavailable)
    }
}

fn principal(name: &str) -> PrincipalRef {
    PrincipalRef::in_domain(
        PrincipalKind::ServiceAccount,
        name,
        meerkat_core::TrustDomainId::new("meta-e1").expect("trust domain"),
    )
    .expect("service principal")
}

fn grant_configuration() -> LocalGrantConfiguration {
    LocalGrantConfiguration {
        root: principal("grant-owner"),
        namespace: EvidenceId::new("meta-e1-grants").expect("namespace"),
        generation: 1,
    }
}

/// A host-built configuration: the host's own grant owner (constructed, not
/// issued from anything by Meta) with refusing policy owners and ingress.
fn host_configuration() -> NativeGrantWorkConfiguration {
    configuration_with(Arc::new(
        LocalGrantAuthority::new(
            grant_configuration(),
            LocalAuthorizationPublication::new(),
            Arc::new(HostAuthorizationClock),
        )
        .expect("host grant owner"),
    ))
}

fn configuration_with(grants: Arc<LocalGrantAuthority>) -> NativeGrantWorkConfiguration {
    let ingress: Arc<meerkat_runtime::meerkat_machine::NativeIngressCheck> =
        Arc::new(|_, _, _, _| -> Result<(), NativeAdmissionError> {
            Err(OperationAuthorizationError::Unavailable.into())
        });
    NativeGrantWorkConfiguration {
        grants,
        ingress,
        invocation_owner: Arc::new(RefusingOwner),
        operation_owner: Arc::new(RefusingOwner),
        retained_resume: None,
        require_scoped_destination: true,
        configured_mcp_attachment: None,
    }
}

/// The worker mob, under a mob id of its own per test: tests in this binary
/// run in parallel in one process, and a shared id makes their supervisors
/// collide on the participant name.
fn definition(test: &str) -> meerkat_mob::MobDefinition {
    meerkat_mob::MobDefinition::from_toml(&format!(
        "[mob]\nid = \"native-grants-{test}\"\n\n[profiles.worker]\nmodel = \"gpt-5.5\"\nruntime_mode = \"turn_driven\"\n\n[profiles.worker.tools]\ncomms = true\n",
    ))
    .expect("mob definition")
}

/// The fake OpenAI key in-process members resolve their provider identity
/// with: the same value the gateway host process below is given, not a
/// secret.
const FIXTURE_OPENAI_KEY: &str = "sk-regression-test";

/// Agent config whose global realm binds OpenAI to [`FIXTURE_OPENAI_KEY`],
/// so a member built in this process never depends on an ambient
/// `OPENAI_API_KEY`. The refusing configuration admits no prompt, so the
/// provider is never called.
fn fixture_provider_config() -> meerkat::Config {
    let mut config = meerkat::Config::default();
    config.realm.insert(
        meerkat_core::connection::GLOBAL_REALM_SLUG.to_string(),
        meerkat_core::RealmConfigSection::from_inline_api_keys(&[("openai", FIXTURE_OPENAI_KEY)]),
    );
    config
}

fn runtime_machine(runtime: &UnifiedRuntime) -> Arc<meerkat_runtime::MeerkatMachine> {
    runtime
        .mob_runtime()
        .session_service()
        .expect("the runtime-backed spec has a session service")
        .acquire_runtime_adapter(None)
        .expect("runtime adapter acquisition")
        .expect("the runtime-backed spec has a canonical runtime machine")
}

#[tokio::test(flavor = "multi_thread")]
async fn a_supplied_grant_owner_is_installed_on_the_constructed_machine() {
    for (persistent, supplied) in [(true, true), (true, false), (false, true)] {
        let dir = tempfile::tempdir().unwrap();
        let mut builder = UnifiedRuntime::builder().definition(definition("supplied"));
        // Persistent state, or the builder's own temporary ephemeral store
        // (a scratch_dir alone selects the identity-first composition).
        if persistent {
            builder = builder.persistent_state(dir.path().join("state"));
        }
        if supplied {
            builder = builder.native_grant_work_configuration(host_configuration());
        }
        let runtime = builder.build().await.expect("build");
        assert_eq!(
            runtime_machine(&runtime).has_native_work_authorization_host(),
            supplied,
            "persistent={persistent} supplied={supplied}"
        );
        let _ = runtime.shutdown().await;
    }
}

/// Set (to the shared directory) only in the first host process of the
/// restart qualification below.
const FIRST_HOST_PROCESS: &str = "META_E1_FIRST_HOST_PROCESS";
const RESTART_TEST: &str = "a_restored_grant_owner_is_installed_after_a_host_restart";

fn owner_store(dir: &std::path::Path) -> Arc<dyn GrantOwnerStore> {
    Arc::new(FileGrantOwnerStore::new(dir.join("grant-owner")))
}

/// The first host process: a new durable owner, the host's administrative
/// issuance (Meta issues nothing), and a Meta runtime with that owner
/// installed. It then exits without teardown, as a restarted process does.
async fn first_host_process(dir: &std::path::Path) -> ! {
    let grants = Arc::new(
        LocalGrantAuthority::new_durable(
            grant_configuration(),
            owner_store(dir),
            LocalAuthorizationPublication::new(),
            Arc::new(HostAuthorizationClock),
        )
        .expect("new durable host owner"),
    );
    let issued = grants
        .issue_root(
            &principal("grant-owner"),
            EvidenceId::new("before-restart").expect("grant id"),
            principal("holder"),
            None,
            ExecutionRestrictions::unrestricted(),
        )
        .expect("host issuance");
    std::fs::write(
        dir.join("issued.json"),
        serde_json::to_vec(&issued).expect("encode reference"),
    )
    .expect("record reference");
    let runtime = UnifiedRuntime::builder()
        .definition(definition("restored"))
        .persistent_state(dir.join("state"))
        .native_grant_work_configuration(configuration_with(grants))
        .build()
        .await
        .expect("first build");
    assert!(runtime_machine(&runtime).has_native_work_authorization_host());
    std::process::exit(0)
}

/// Embedder restart qualification: the host's durable owner survives a host
/// process restart. The restarted host restores the same owner from its store
/// (never new issuance), the reference issued before the restart resolves
/// unchanged, and the restored owner is installed on the new Meta machine over
/// the same persistent state. An empty store restores nothing: there is no
/// fallback to fresh issuance.
#[tokio::test(flavor = "multi_thread")]
async fn a_restored_grant_owner_is_installed_after_a_host_restart() {
    if let Some(dir) = std::env::var_os(FIRST_HOST_PROCESS) {
        first_host_process(std::path::Path::new(&dir)).await;
    }
    let dir = tempfile::tempdir().unwrap();
    let restore = || {
        LocalGrantAuthority::restore(
            owner_store(dir.path()),
            LocalAuthorizationPublication::new(),
            Arc::new(HostAuthorizationClock),
        )
    };
    assert!(
        matches!(restore(), Err(GrantDurabilityError::Missing)),
        "an empty store has no owner to restore"
    );

    let first = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", RESTART_TEST, "--nocapture"])
        .env(FIRST_HOST_PROCESS, dir.path())
        .status()
        .expect("first host process");
    assert!(first.success(), "first host process: {first}");
    let issued: GrantLineageRef =
        serde_json::from_slice(&std::fs::read(dir.path().join("issued.json")).unwrap())
            .expect("recorded reference");

    let restored = Arc::new(restore().expect("the restarted host restores its retained owner"));
    let resolved = restored
        .resolve_lineage(std::slice::from_ref(&issued), &principal("holder"), None)
        .expect("the pre-restart reference resolves on the restored owner");
    assert_eq!(resolved.check_current(), Ok(()));
    let second = UnifiedRuntime::builder()
        .definition(definition("restored"))
        .persistent_state(dir.path().join("state"))
        .native_grant_work_configuration(configuration_with(restored))
        .build()
        .await
        .expect("restarted build");
    assert!(runtime_machine(&second).has_native_work_authorization_host());
    let _ = second.shutdown().await;
}

/// Set (to `persistent`, `ephemeral` or `error`) only in the gateway host
/// process of the test below; `GATEWAY_RECORD` names its record file.
const GATEWAY_HOST_PROCESS: &str = "META_E1_GATEWAY_HOST_PROCESS";
const GATEWAY_RECORD: &str = "META_E1_GATEWAY_RECORD";
const GATEWAY_TEST: &str = "the_gateway_host_grant_owner_is_installed_before_bootstrap";

/// A gateway host that supplies (or fails to supply) its grant owner and
/// records whether the gateway's runtime machine carries it. It refuses the
/// Ready publication: the record is all this test needs.
struct GrantHost {
    supply: bool,
    record: std::path::PathBuf,
}

#[async_trait::async_trait]
impl meerkat_meta::rpc_gateway::GatewayHostComposition for GrantHost {
    async fn prepare_bootstrap(
        &self,
        spec: meerkat_meta::MobBootstrapSpec,
    ) -> Result<meerkat_meta::MobBootstrapSpec, meerkat_meta::rpc_gateway::GatewayHostError> {
        Ok(spec)
    }

    fn native_grant_work_configuration(
        &self,
    ) -> Result<Option<NativeGrantWorkConfiguration>, meerkat_meta::rpc_gateway::GatewayHostError>
    {
        let mut entered = self.record.clone().into_os_string();
        entered.push(".supply");
        std::fs::write(entered, "entered").expect("record the supply callback");
        if self.supply {
            Ok(Some(host_configuration()))
        } else {
            Err(meerkat_meta::rpc_gateway::GatewayHostError::Unavailable)
        }
    }

    async fn bind_runtime(
        &self,
        runtime: &mut UnifiedRuntime,
    ) -> Result<(), meerkat_meta::rpc_gateway::GatewayHostError> {
        let installed = runtime_machine(runtime).has_native_work_authorization_host();
        std::fs::write(&self.record, format!("installed={installed}")).expect("record");
        Ok(())
    }

    fn publish_bootstrap_ready(
        &self,
        _publication: meerkat_meta::rpc_gateway::GatewayBootstrapReady<'_>,
    ) -> Result<(), meerkat_meta::rpc_gateway::GatewayHostError> {
        Err(meerkat_meta::rpc_gateway::GatewayHostError::Denied)
    }

    fn begin_close(
        &self,
        _reason: meerkat_meta::rpc_gateway::GatewayHostCloseReason,
    ) -> Result<(), meerkat_meta::rpc_gateway::GatewayHostError> {
        Ok(())
    }

    async fn join_close(&self) -> Result<(), meerkat_meta::rpc_gateway::GatewayHostError> {
        Ok(())
    }

    async fn wait_closed(&self) {
        std::future::pending::<()>().await;
    }
}

/// D50 S1 on the composed gateway (`run_persistent_with_host`, the path a
/// downstream host uses): the host's grant owner is taken before the gateway
/// machine exists and is installed on it, on the persistent and the
/// ephemeral path. A host that cannot supply its owner fails init before any
/// runtime is bound. Each case is a separate gateway process over real stdio.
#[test]
fn the_gateway_host_grant_owner_is_installed_before_bootstrap() {
    use std::io::Write;
    if let Some(mode) = std::env::var_os(GATEWAY_HOST_PROCESS) {
        let host = Arc::new(GrantHost {
            supply: mode != "error",
            record: std::env::var_os(GATEWAY_RECORD)
                .expect("record path")
                .into(),
        });
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("tokio runtime");
        let _ = runtime.block_on(meerkat_meta::rpc_gateway::run_persistent_with_host(
            None, host,
        ));
        std::process::exit(0);
    }
    for mode in ["persistent", "ephemeral", "error"] {
        let dir = tempfile::tempdir().unwrap();
        let record = dir.path().join("record");
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", GATEWAY_TEST, "--nocapture"])
            .env(GATEWAY_HOST_PROCESS, mode)
            .env(GATEWAY_RECORD, &record)
            .env("OPENAI_API_KEY", FIXTURE_OPENAI_KEY)
            .env("XDG_STATE_HOME", dir.path().join("xdg-state"))
            .current_dir(dir.path())
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .expect("gateway host process");
        let stdout = child.stdout.take().expect("stdout");
        let output = std::thread::spawn(move || {
            let mut text = String::new();
            let _ = std::io::Read::read_to_string(&mut { stdout }, &mut text);
            text
        });
        let mut params = serde_json::json!({
            "mob_config": "[mob]\nid = \"native-grants-gateway\"\n\n[profiles.worker]\nmodel = \"gpt-5.5\"\n",
        });
        if mode != "ephemeral" {
            params["persistent_state"] = serde_json::json!(dir.path().join("state"));
        }
        let mut stdin = child.stdin.take().expect("stdin");
        writeln!(
            stdin,
            "{}",
            serde_json::json!({"jsonrpc": "2.0", "id": "init", "method": "meta/init", "params": params})
        )
        .expect("write init");
        stdin.flush().expect("flush init");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(90);
        while child.try_wait().unwrap().is_none() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        drop(stdin);
        let exited = child.try_wait().unwrap();
        if exited.is_none() {
            let _ = child.kill();
        }
        let _ = child.wait();
        let recorded = std::fs::read_to_string(&record).ok();
        let init_response = output
            .join()
            .expect("stdout reader")
            .lines()
            .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
            .find(|message| message["id"] == "init");
        match mode {
            "error" => {
                assert!(
                    dir.path().join("record.supply").exists(),
                    "the gateway entered the host's supply callback"
                );
                assert_eq!(recorded, None, "an unavailable owner binds no runtime");
                assert_eq!(
                    init_response
                        .as_ref()
                        .map(|response| &response["error"]["message"]),
                    Some(&serde_json::json!(
                        meerkat_meta::rpc_gateway::GatewayHostError::Unavailable.to_string()
                    )),
                    "the init failure is the host's own error: {init_response:?}"
                );
            }
            _ => assert_eq!(recorded.as_deref(), Some("installed=true"), "{mode}"),
        }
        assert!(
            exited.is_some(),
            "{mode}: the gateway ends after its refused init"
        );
    }
}

/// A shut-down and dropped runtime releases its machine and the host's grant
/// owner, so the same process can restore that owner: its store lease is held
/// until the owner drops. Covers the persistent and ephemeral compositions and
/// a definition whose profile enables image generation (the factory then
/// retains the machine as well).
#[tokio::test(flavor = "multi_thread")]
async fn a_shut_down_runtime_releases_its_machine_and_grant_owner() {
    let image_generation = meerkat_mob::MobDefinition::from_toml(
        "[mob]\nid = \"native-grants-images\"\n\n[profiles.worker]\nmodel = \"gpt-5.5\"\nruntime_mode = \"turn_driven\"\n\n[profiles.worker.tools]\ncomms = true\nimage_generation = true\n",
    )
    .expect("image generation definition");
    for (case, persistent, definition) in [
        ("persistent", true, definition("shut-down")),
        ("ephemeral", false, definition("shut-down")),
        ("image-generation", true, image_generation),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let grants = Arc::new(
            LocalGrantAuthority::new_durable(
                grant_configuration(),
                owner_store(dir.path()),
                LocalAuthorizationPublication::new(),
                Arc::new(HostAuthorizationClock),
            )
            .expect("new durable host owner"),
        );
        let owner = Arc::downgrade(&grants);
        let mut builder = UnifiedRuntime::builder()
            .definition(definition)
            .native_grant_work_configuration(configuration_with(grants));
        if persistent {
            builder = builder.persistent_state(dir.path().join("state"));
        }
        let runtime = builder.build().await.expect("build");
        let machine = Arc::downgrade(&runtime_machine(&runtime));
        let _ = runtime.shutdown().await;
        drop(runtime);
        for _ in 0..50 {
            if machine.strong_count() == 0 && owner.strong_count() == 0 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        assert_eq!(
            (machine.strong_count(), owner.strong_count()),
            (0, 0),
            "{case}: (machine, grant owner) references after shutdown"
        );
        LocalGrantAuthority::restore(
            owner_store(dir.path()),
            LocalAuthorizationPublication::new(),
            Arc::new(HostAuthorizationClock),
        )
        .unwrap_or_else(|error| panic!("{case}: in-process restore: {error}"));
    }
}

fn durable_owner(dir: &std::path::Path) -> Arc<LocalGrantAuthority> {
    Arc::new(
        LocalGrantAuthority::new_durable(
            grant_configuration(),
            owner_store(dir),
            LocalAuthorizationPublication::new(),
            Arc::new(HostAuthorizationClock),
        )
        .expect("new durable host owner"),
    )
}

/// Wait (bounded) for aborted work to unwind, then assert nothing retains
/// the owner, and restore it from its store in this process.
async fn assert_released_and_restorable(
    case: &str,
    owner: &std::sync::Weak<LocalGrantAuthority>,
    dir: &std::path::Path,
) {
    let held = || owner.strong_count();
    for _ in 0..50 {
        if held() == 0 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert_eq!(held(), 0, "{case}: grant owner references");
    LocalGrantAuthority::restore(
        owner_store(dir),
        LocalAuthorizationPublication::new(),
        Arc::new(HostAuthorizationClock),
    )
    .unwrap_or_else(|error| panic!("{case}: in-process restore: {error}"));
}

/// Decision 67. Dropping the last public owner of an IDLE composition without
/// `shutdown()` hands the runtime to its retained teardown; the native
/// lifecycle owner closes the mob's admission atomically (idle only), and the
/// existing awaited shutdown then releases the machine and the host's grant
/// owner: the store restores in the same process and the mob id is free.
#[tokio::test(flavor = "multi_thread")]
async fn dropping_the_last_public_owner_of_an_idle_runtime_settles_it() {
    let dir = tempfile::tempdir().unwrap();
    let grants = durable_owner(dir.path());
    let owner = Arc::downgrade(&grants);
    let runtime = UnifiedRuntime::builder()
        .definition(definition("idle-drop"))
        .persistent_state(dir.path().join("state"))
        .native_grant_work_configuration(configuration_with(grants))
        .build()
        .await
        .expect("build");
    let machine = Arc::downgrade(&runtime_machine(&runtime));
    // A watch receiver holds no mob capability or machine.
    let closed = runtime.mob_handle().observer().admission_closed();
    drop(runtime);
    assert_released_and_restorable("idle drop", &owner, dir.path()).await;
    assert_eq!(machine.strong_count(), 0, "the machine is released as well");
    assert!(
        matches!(
            *closed.borrow(),
            Some(meerkat_mob::MobAdmissionCloseReason::LastOwnerReleased)
        ),
        "the native owner closed admission for the released last owner"
    );
    let successor = UnifiedRuntime::builder()
        .definition(definition("idle-drop"))
        .persistent_state(dir.path().join("successor"))
        .build()
        .await
        .expect("the settled mob's id is free again");
    let _ = successor.shutdown().await;
}

/// Admission at the closing boundary: a non-owning observer retained across
/// the drop does not keep the mob owned; once the native owner has closed
/// admission, a mutation through it is refused, never admitted into the
/// settling mob. Releasing the observer lets the settlement finish.
#[tokio::test(flavor = "multi_thread")]
async fn a_mutation_after_the_idle_close_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let grants = durable_owner(dir.path());
    let owner = Arc::downgrade(&grants);
    let runtime = UnifiedRuntime::builder()
        .definition(definition("idle-mutation"))
        .persistent_state(dir.path().join("state"))
        .native_grant_work_configuration(configuration_with(grants))
        .build()
        .await
        .expect("build");
    let observer = runtime.mob_handle().observer();
    let mut closed = observer.admission_closed();
    drop(runtime);
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while closed.borrow_and_update().is_none() {
            if closed.changed().await.is_err() {
                break;
            }
        }
    })
    .await
    .expect("the idle mob's admission closes");
    let refused = observer
        .ensure_member(meerkat_mob::SpawnMemberSpec::new(
            meerkat_mob::ProfileName::from("worker"),
            meerkat_mob::ids::AgentIdentity::from("late"),
        ))
        .await;
    assert!(
        matches!(
            refused,
            Err(meerkat_mob::MobError::AdmissionClosed { .. }
                | meerkat_mob::MobError::ActorCommandChannelClosed)
        ),
        "a mutation after the close is refused: {refused:?}"
    );
    drop(observer);
    assert_released_and_restorable("observer released", &owner, dir.path()).await;
}

/// A public handle retained from the runtime keeps the mob publicly owned:
/// dropping the runtime closes nothing, the owner stays held and same-store
/// restore is refused, and admission stays open for that handle. Releasing
/// the handle then settles the idle composition.
#[tokio::test(flavor = "multi_thread")]
async fn a_retained_public_handle_keeps_a_dropped_runtime_owned() {
    let dir = tempfile::tempdir().unwrap();
    let grants = durable_owner(dir.path());
    let owner = Arc::downgrade(&grants);
    let runtime = UnifiedRuntime::builder()
        .definition(definition("retained-handle"))
        .persistent_state(dir.path().join("state"))
        .native_grant_work_configuration(configuration_with(grants))
        .build()
        .await
        .expect("build");
    let retained = runtime.mob_handle();
    drop(runtime);
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    assert!(
        owner.strong_count() > 0,
        "the retained handle keeps the owner"
    );
    assert!(
        matches!(
            LocalGrantAuthority::restore(
                owner_store(dir.path()),
                LocalAuthorizationPublication::new(),
                Arc::new(HostAuthorizationClock),
            ),
            Err(GrantDurabilityError::StoreUnavailable)
        ),
        "same-store restore is refused while a public owner remains"
    );
    assert!(
        retained.admission_closed().borrow().is_none(),
        "admission stays open for the retained public handle"
    );
    drop(retained);
    assert_released_and_restorable("released handle", &owner, dir.path()).await;
}

/// Blocks every model call in the provider, so a member's first turn stays
/// in flight: entered work the native owner counts as busy.
struct PendingClient;

impl meerkat_client::LlmClient for PendingClient {
    fn project_replay_messages(
        &self,
        messages: &[meerkat_core::Message],
    ) -> Result<Vec<meerkat_core::Message>, meerkat_client::LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(
        &'a self,
        _request: &'a meerkat_client::LlmRequest,
    ) -> std::pin::Pin<
        Box<
            dyn futures::Stream<Item = Result<meerkat_client::LlmEvent, meerkat_client::LlmError>>
                + Send
                + 'a,
        >,
    > {
        Box::pin(futures::stream::pending())
    }

    fn provider(&self) -> meerkat::Provider {
        meerkat::Provider::OpenAI
    }

    fn health_check<'life0, 'async_trait>(
        &'life0 self,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = Result<(), meerkat_client::LlmError>>
                + Send
                + 'async_trait,
        >,
    >
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async { Ok(()) })
    }
}

/// Entered work keeps the composition: with a member turn in flight,
/// dropping the last public owner does not close admission, and the machine
/// stays held.
#[tokio::test(flavor = "multi_thread")]
async fn a_dropped_runtime_with_entered_work_stays_open() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = UnifiedRuntime::builder()
        .definition(
            meerkat_mob::MobDefinition::from_toml(
                "[mob]\nid = \"native-grants-busy\"\n\n[profiles.worker]\nmodel = \"gpt-5.5\"\n\n[profiles.worker.tools]\ncomms = true\n",
            )
            .expect("definition"),
        )
        .persistent_state(dir.path().join("state"))
        .default_llm_client(Arc::new(PendingClient))
        .build()
        .await
        .expect("build");
    runtime
        .mob_handle()
        .ensure_member(
            meerkat_mob::SpawnMemberSpec::new(
                meerkat_mob::ProfileName::from("worker"),
                meerkat_mob::ids::AgentIdentity::from("worker-1"),
            )
            .with_initial_message("start"),
        )
        .await
        .expect("a member with a first turn enters the composition");
    let closed = runtime.mob_handle().observer().admission_closed();
    let machine = Arc::downgrade(&runtime_machine(&runtime));
    drop(runtime);
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    assert!(machine.strong_count() > 0, "entered work keeps the machine");
    assert!(
        closed.borrow().is_none(),
        "a busy mob's admission is not closed"
    );
}

/// A fully composed candidate refused because its mob id is still served by
/// a live incumbent (native NameOccupied) releases its own composition and
/// grant owner; the incumbent keeps its owner. Separate grant stores.
#[tokio::test(flavor = "multi_thread")]
async fn a_candidate_refused_for_an_occupied_mob_id_releases_its_grant_owner() {
    let mob_id = format!("native-grants-occupied-{}", std::process::id());
    let occupied_definition = || {
        meerkat_mob::MobDefinition::from_toml(&format!(
            "[mob]\nid = \"{mob_id}\"\n\n[profiles.worker]\nmodel = \"gpt-5.5\"\nruntime_mode = \"turn_driven\"\n\n[profiles.worker.tools]\ncomms = true\n",
        ))
        .expect("definition")
    };
    let incumbent_dir = tempfile::tempdir().unwrap();
    let incumbent = UnifiedRuntime::builder()
        .definition(occupied_definition())
        .native_grant_work_configuration(configuration_with(durable_owner(incumbent_dir.path())))
        .build()
        .await
        .expect("incumbent build");

    let candidate_dir = tempfile::tempdir().unwrap();
    let grants = durable_owner(candidate_dir.path());
    let owner = Arc::downgrade(&grants);
    let error = UnifiedRuntime::builder()
        .definition(occupied_definition())
        .native_grant_work_configuration(configuration_with(grants))
        .build()
        .await
        .err()
        .expect("the candidate is refused while the incumbent is live");
    assert!(
        error.to_string().contains("already has a live route"),
        "the native NameOccupied refusal: {error}"
    );
    assert_released_and_restorable("candidate", &owner, candidate_dir.path()).await;

    assert!(
        runtime_machine(&incumbent).has_native_work_authorization_host(),
        "the incumbent keeps its owner"
    );
    let _ = incumbent.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_grant_owner_with_a_prebuilt_spec_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let spec = meerkat_meta::MobBootstrapSpec::ephemeral(
        definition("prebuilt-spec"),
        meerkat_mob::MobStorage::in_memory(),
        dir.path().to_path_buf(),
        4,
        None,
    )
    .expect("prebuilt spec");
    let error = UnifiedRuntime::builder()
        .mob_spec(spec)
        .native_grant_work_configuration(host_configuration())
        .module_config(meerkat_meta::MetaConfig {
            modules: vec![],
            discovery: meerkat_meta::DiscoverySpec {
                namespace: "native-grants".to_string(),
                modules: vec![],
            },
            pre_spawn: vec![],
        })
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .await
        .err()
        .expect("a prebuilt spec cannot take a grant owner");
    assert!(
        error
            .to_string()
            .contains("native_grant_work_configuration"),
        "{error}"
    );
}

/// The host's application policy owner: one provider whose policy allows
/// every tool, counting each policy snapshot a member construction binds.
struct HostPolicy {
    id: meerkat_core::PolicyProviderId,
    snapshots: Arc<std::sync::atomic::AtomicUsize>,
}

struct AllowsEverything;

impl meerkat_core::ToolConsequencePolicySnapshot for AllowsEverything {
    fn provenance(&self) -> meerkat_core::PolicyEvaluationProvenance {
        meerkat_core::PolicyEvaluationProvenance {
            revision: meerkat_core::PolicyRevision(1),
            digest: meerkat_core::PolicyDigest::from_canonical_bytes(b"allows-everything"),
        }
    }

    fn evaluate(
        &self,
        _request: &meerkat_core::ToolConsequenceRequest,
    ) -> meerkat_core::ToolConsequenceVerdict {
        meerkat_core::ToolConsequenceVerdict::Allow
    }
}

impl meerkat_core::ToolConsequenceNarrowingPolicy for HostPolicy {
    fn provider_id(&self) -> &meerkat_core::PolicyProviderId {
        &self.id
    }

    fn generation(&self) -> meerkat_core::PolicyProviderGeneration {
        meerkat_core::PolicyProviderGeneration(1)
    }

    fn snapshot(
        &self,
        _policy_id: &meerkat_core::PolicyId,
    ) -> Result<
        Arc<dyn meerkat_core::ToolConsequencePolicySnapshot>,
        meerkat_core::ToolConsequenceFailure,
    > {
        self.snapshots
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(Arc::new(AllowsEverything))
    }
}

fn host_registry(
    snapshots: &Arc<std::sync::atomic::AtomicUsize>,
) -> Arc<meerkat_core::ToolConsequencePolicyRegistry> {
    Arc::new(
        meerkat_core::ToolConsequencePolicyRegistry::new(
            vec![Arc::new(HostPolicy {
                id: meerkat_core::PolicyProviderId::new("host").expect("provider id"),
                snapshots: Arc::clone(snapshots),
            })],
            meerkat_core::PolicyEvaluationSupervisorConfig::default(),
            None,
        )
        .expect("host policy registry"),
    )
}

/// Records each member registered through it, selects the member's own
/// session document as its context, and selects the host's policy for the
/// member's actual prepared surface.
#[derive(Default)]
struct RecordingHook {
    selected: std::sync::Mutex<Vec<String>>,
    prepared: std::sync::Mutex<Vec<String>>,
}

#[async_trait::async_trait]
impl meerkat_mob::MemberRuntimeRegistrationHook for RecordingHook {
    async fn select_destination(
        &self,
        identity: &meerkat_mob::MemberRuntimeBuildIdentity,
    ) -> Result<meerkat_mob::MemberDestinationSelection, meerkat_mob::MobError> {
        self.selected
            .lock()
            .unwrap()
            .push(identity.member().member.clone());
        Ok(selection(identity))
    }

    async fn expected_destination(
        &self,
        identity: &meerkat_mob::MemberRuntimeBuildIdentity,
    ) -> Result<meerkat_mob::MemberDestinationSelection, meerkat_mob::MobError> {
        Ok(selection(identity))
    }

    /// No installed-provider claims: any surface is acceptable to this host.
    async fn verify_member_surface(
        &self,
        _identity: &meerkat_mob::MemberRuntimeBuildIdentity,
        _surface: &[meerkat_core::ToolDef],
    ) -> Result<(), meerkat_mob::MobError> {
        Ok(())
    }

    /// The host's resolved policy, compiled against exactly the surface the
    /// member's construction prepared.
    async fn prepare_member_surface(
        &self,
        identity: &meerkat_mob::MemberRuntimeBuildIdentity,
        surface: &meerkat_core::PreparedToolSurface,
    ) -> Result<meerkat_core::ToolSurfaceSelection, meerkat_mob::MobError> {
        self.prepared
            .lock()
            .unwrap()
            .push(identity.member().member.clone());
        Ok(meerkat_core::ToolSurfaceSelection {
            binding: meerkat_core::ApplicationToolPolicyBinding::Provider {
                provider_id: meerkat_core::PolicyProviderId::new("host").expect("provider id"),
                policy_id: meerkat_core::PolicyId::new("member-tools").expect("policy id"),
            },
            compiled_against: surface.clone(),
        })
    }
}

fn selection(
    identity: &meerkat_mob::MemberRuntimeBuildIdentity,
) -> meerkat_mob::MemberDestinationSelection {
    let owner = PrincipalRef::in_domain(
        PrincipalKind::SharedAgent,
        identity.member().member.as_str(),
        meerkat_core::TrustDomainId::new("meta-e1").expect("trust domain"),
    )
    .expect("member principal");
    meerkat_mob::MemberDestinationSelection {
        logical_owner: owner.clone(),
        context: identity.session_document().clone(),
        context_generation: 1,
        audience: meerkat_mob::AudienceRef::Principal { principal: owner },
    }
}

/// E1 + S2 on the builder path an embedder uses: with the host's grant owner
/// installed, a hooked member's selected destination installs on its exact
/// registration (S3 refuses that install without the governed work host).
/// Its construction is prepared once, its host selects the host policy for
/// that actual surface, and it is finished bound to that policy through the
/// host's own registry. The member is turn-driven: this refusing
/// configuration admits no turn, so no first prompt is admitted at spawn.
#[tokio::test(flavor = "multi_thread")]
async fn a_hooked_member_registers_on_a_machine_with_the_host_grant_owner() {
    let dir = tempfile::tempdir().unwrap();
    let hook = Arc::new(RecordingHook::default());
    let snapshots = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let runtime = UnifiedRuntime::builder()
        .definition(definition("hooked-member"))
        .meerkat_config(fixture_provider_config())
        .persistent_state(dir.path().join("state"))
        .native_grant_work_configuration(host_configuration())
        .member_runtime_registration_hook(hook.clone())
        .tool_consequence_policy_registry(host_registry(&snapshots))
        .build()
        .await
        .expect("build");
    runtime
        .mob_handle()
        .ensure_member(meerkat_mob::SpawnMemberSpec::new(
            meerkat_mob::ProfileName::from("worker"),
            meerkat_mob::ids::AgentIdentity::from("worker-1"),
        ))
        .await
        .expect("a hooked member registers on the governed machine");
    assert_eq!(hook.selected.lock().unwrap().len(), 1);
    assert_eq!(
        hook.prepared.lock().unwrap().as_slice(),
        ["worker-1".to_string()],
        "the member's one construction is decided by its host"
    );
    assert!(
        snapshots.load(std::sync::atomic::Ordering::SeqCst) == 1,
        "the member is finished bound to the selected host policy"
    );
    let _ = runtime.shutdown().await;
}

/// A reviewed attachment control for exactly one server, deciding its live
/// apply (persistence is never requested here).
struct ApplyControl {
    server: meerkat_core::mcp_config::McpServerConfig,
    apply: Result<(), meerkat_core::mcp_attachment::McpAttachmentRefusal>,
}

impl meerkat_core::mcp_attachment::McpAttachmentAuthority for ApplyControl {
    fn server(&self) -> &meerkat_core::mcp_config::McpServerConfig {
        &self.server
    }

    fn authorize_apply(&self) -> Result<(), meerkat_core::mcp_attachment::McpAttachmentRefusal> {
        self.apply.clone()
    }

    fn authorize_persist(
        &self,
        _scope: meerkat_core::mcp_config::McpScope,
    ) -> Result<(), meerkat_core::mcp_attachment::McpAttachmentRefusal> {
        Err(meerkat_core::mcp_attachment::McpAttachmentRefusal::Denied)
    }
}

/// A stdio "server" whose only effect is touching `marker` when started.
fn sentinel(name: &str, marker: &std::path::Path) -> meerkat_core::mcp_config::McpServerConfig {
    meerkat_core::mcp_config::McpServerConfig::stdio(
        name,
        "/bin/sh",
        vec!["-c".into(), format!("touch '{}'", marker.display())],
        std::collections::HashMap::new(),
    )
}

/// A stdio MCP server that answers `initialize` and lists one tool,
/// `lookup`, until its stdin closes.
fn listing_server(name: &str) -> meerkat_core::mcp_config::McpServerConfig {
    let script = r#"while IFS= read -r line; do
  id=$(printf '%s\n' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
  case "$line" in
    *'"method":"initialize"'*) printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":"2025-03-26","capabilities":{"tools":{}},"serverInfo":{"name":"listing","version":"0"}}}\n' "$id" ;;
    *'"method":"tools/list"'*) printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[{"name":"lookup","inputSchema":{"type":"object"}}]}}\n' "$id" ;;
  esac
done"#;
    meerkat_core::mcp_config::McpServerConfig::stdio(
        name,
        "/bin/sh",
        vec!["-c".into(), script.to_string()],
        std::collections::HashMap::new(),
    )
}

fn live_only(
    server: meerkat_core::mcp_config::McpServerConfig,
    apply: Result<(), meerkat_core::mcp_attachment::McpAttachmentRefusal>,
) -> meerkat_core::mcp_attachment::LiveMcpAttachment {
    meerkat_core::mcp_attachment::LiveMcpAttachment {
        control: Arc::new(ApplyControl {
            server: server.clone(),
            apply,
        }),
        server,
        persist: None,
    }
}

async fn spawned(marker: &std::path::Path) -> bool {
    for _ in 0..50 {
        if marker.exists() {
            return true;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    false
}

/// Member live MCP attachment on the host's governed machine: a member built
/// with no configured server owns its router there, so Meta's session owner
/// attaches the member's first server through the member's own actor. The
/// allowed server starts; a denied apply stages but starts nothing.
#[tokio::test(flavor = "multi_thread")]
async fn a_governed_member_attaches_its_first_server_through_its_session_owner() {
    let dir = tempfile::tempdir().unwrap();
    let markers = tempfile::tempdir().unwrap();
    let hook = Arc::new(RecordingHook::default());
    let snapshots = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let runtime = UnifiedRuntime::builder()
        .definition(definition("member-live-mcp"))
        .meerkat_config(fixture_provider_config())
        .persistent_state(dir.path().join("state"))
        .native_grant_work_configuration(host_configuration())
        .member_runtime_registration_hook(hook)
        .tool_consequence_policy_registry(host_registry(&snapshots))
        .build()
        .await
        .expect("build");
    runtime
        .mob_handle()
        .ensure_member(meerkat_mob::SpawnMemberSpec::new(
            meerkat_mob::ProfileName::from("worker"),
            meerkat_mob::ids::AgentIdentity::from("worker-1"),
        ))
        .await
        .expect("a hooked member registers on the governed machine");
    let sessions = runtime
        .mob_runtime()
        .session_service()
        .expect("the runtime-backed spec has a session service")
        .list(meerkat_core::service::SessionQuery::default())
        .await
        .expect("sessions");
    assert_eq!(sessions.len(), 1, "the member's one session");
    let session_id = sessions[0].session_id.clone();
    let owner = runtime
        .mob_runtime()
        .controller_pin_source()
        .expect("the runtime records its session owner");

    let first = markers.path().join("first");
    let receipt = owner
        .try_attach_live_mcp_server(&session_id, live_only(sentinel("first", &first), Ok(())))
        .await
        .expect("the governed member attaches");
    assert_eq!(
        receipt.outcome,
        meerkat_core::mcp_attachment::McpAttachmentOutcome {
            persisted: false,
            staged: true,
        }
    );
    assert!(spawned(&first).await, "the allowed first server starts");
    // The sentinel starts but never answers the MCP handshake: its exact
    // attempt is that attempt's own failure, never usable.
    let observed = receipt
        .attempt
        .wait("lookup", std::time::Duration::from_secs(40))
        .await;
    assert!(
        matches!(
            observed,
            meerkat_core::mcp_attachment::McpAttachmentReadiness::Failed { .. }
        ),
        "{observed:?}"
    );

    // A server that answers the handshake and lists one tool: its exact
    // attempt becomes usable for that operation only.
    let tools = owner
        .try_attach_live_mcp_server(&session_id, live_only(listing_server("tools"), Ok(())))
        .await
        .expect("the governed member attaches a second server");
    let ready = tools
        .attempt
        .wait("lookup", std::time::Duration::from_secs(40))
        .await;
    assert!(
        matches!(
            ready,
            meerkat_core::mcp_attachment::McpAttachmentReadiness::Usable { .. }
        ),
        "{ready:?}"
    );
    assert!(matches!(
        tools.attempt.observe("missing").await,
        meerkat_core::mcp_attachment::McpAttachmentReadiness::ToolNotRouted { .. }
    ));

    let denied = markers.path().join("denied");
    assert!(
        owner
            .try_attach_live_mcp_server(
                &session_id,
                live_only(
                    sentinel("denied", &denied),
                    Err(meerkat_core::mcp_attachment::McpAttachmentRefusal::Denied),
                ),
            )
            .await
            .is_ok(),
        "staging is not a connection claim"
    );
    assert!(!spawned(&denied).await, "a denied apply starts nothing");
    let _ = runtime.shutdown().await;
}
