//! Every Meta member builder waits for a member's configured MCP servers
//! (Core's `wait_for_mcp`) before its construction is prepared, so the tool
//! surface its host decides on is captured once those servers have connected
//! or failed, never while they are pending. On the composed stdio gateway,
//! a server the member's profile configures reaches that surface with its MCP
//! provenance. On the library builder, persistent and ephemeral, a member
//! whose server fails still spawns: its session holds the typed setup-failure
//! notice, and its surface carries the healthy server's tool and nothing of
//! the failed one. A member of the plain public ephemeral spec is built once
//! its slow server has connected, so its first request already offers that
//! server's tool. A server that never answers its handshake delays the first
//! turn only up to its own connect timeout, plus the at most 0.25 s EOF grace
//! its close gets. The governed host here (a grant owner, a destination and a
//! policy per member) allows every configured server.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use meerkat_authorization::clock::HostAuthorizationClock;
use meerkat_authorization::grant_policy::{
    AdmittedWorkPolicyOwner, OperationPolicyOwner, WorkOwnerAllowance,
};
use meerkat_authorization::grants::{LocalGrantAuthority, LocalGrantConfiguration};
use meerkat_authorization::policy::{LocalPolicyAllowance, LocalPolicyPurpose};
use meerkat_authorization::publication::LocalAuthorizationPublication;
use meerkat_authorization_contracts::evidence::EvidenceId;
use meerkat_authorization_contracts::work_association::InputAuthorityAssociation;
use meerkat_core::auth::{PrincipalKind, PrincipalRef};
use meerkat_core::mcp_attachment::{ConfiguredMcpAttachmentAuthority, McpAttachmentRefusal};
use meerkat_core::{OperationAuthorizationError, PreparedAuthorizationBinding};
use meerkat_meta::UnifiedRuntime;
use meerkat_meta::rpc_gateway::{
    GatewayBootstrapReady, GatewayHostCloseReason, GatewayHostComposition, GatewayHostError,
};
use meerkat_runtime::input_authority::NativeAdmissionError;
use meerkat_runtime::meerkat_machine::{
    ConfiguredMcpAttachmentProvider, NativeGrantWorkConfiguration, NativeIngressCheck,
    RuntimeSessionRegistrationWitness, SessionAgentBinding,
};
use serde_json::{Value, json};

#[path = "support/llm_usage.rs"]
mod llm_usage;

/// Set only in the gateway host process; `RECORD` names its record file.
const HOST_PROCESS: &str = "META_MEMBER_MCP_SURFACE_HOST";
const RECORD: &str = "META_MEMBER_MCP_SURFACE_RECORD";
const GATEWAY_TEST: &str = "a_profile_mcp_server_reaches_the_prepared_surface_on_the_stdio_gateway";
const BACKSTOP: Duration = Duration::from_mins(3);
/// The fake OpenAI key members resolve their provider identity with: these
/// members are built and decided on, never run, so it is never sent.
const FIXTURE_OPENAI_KEY: &str = "sk-regression-test";
/// The healthy server's name and the one tool it lists.
const LISTING: &str = "listing";
const LOOKUP: &str = "lookup";
/// A server that exits before its handshake.
const BROKEN: &str = "broken";

/// A stdio MCP server that answers `initialize` and lists one tool,
/// [`LOOKUP`], until its stdin closes.
const LISTING_SERVER: &str = r#"while IFS= read -r line; do
  id=$(printf '%s\n' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
  case "$line" in
    *'"method":"initialize"'*) printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":"2025-03-26","capabilities":{"tools":{}},"serverInfo":{"name":"listing","version":"0"}}}\n' "$id" ;;
    *'"method":"tools/list"'*) printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[{"name":"lookup","inputSchema":{"type":"object"}}]}}\n' "$id" ;;
  esac
done
"#;

/// Write [`LISTING_SERVER`] into `dir` and answer its path.
fn listing_server(dir: &Path) -> PathBuf {
    let path = dir.join("listing.sh");
    std::fs::write(&path, LISTING_SERVER).expect("write the listing server");
    path
}

/// A mob whose turn-driven `worker` profile configures the listing server at
/// `listing` and, with `broken`, a second server that exits at once.
fn mob_config(mob_id: &str, listing: &Path, broken: bool) -> String {
    let mut config = format!(
        r#"
[mob]
id = "{mob_id}"

[profiles.worker]
model = "gpt-5.5"
runtime_mode = "turn_driven"

[profiles.worker.tools]
comms = true

[[profiles.worker.tools.mcp_servers]]
name = "{LISTING}"
command = "/bin/sh"
args = [{listing:?}]
"#,
        listing = listing.display().to_string(),
    );
    if broken {
        config.push_str(&format!(
            r#"
[[profiles.worker.tools.mcp_servers]]
name = "{BROKEN}"
command = "/bin/sh"
args = ["-c", "exit 3"]
"#
        ));
    }
    config
}

/// Refuses every governed turn and operation: these members are built and
/// decided on, never run.
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

/// Allows each configured server it is asked about.
struct AllowConfigured;

impl ConfiguredMcpAttachmentAuthority for AllowConfigured {
    fn authorize_configured(
        &self,
        _server: &meerkat_core::McpServerConfig,
    ) -> Result<(), McpAttachmentRefusal> {
        Ok(())
    }
}

/// Decides the configured servers of every member registration: all of them
/// are allowed.
struct AllowEveryConfiguredServer;

impl ConfiguredMcpAttachmentProvider for AllowEveryConfiguredServer {
    fn configured(
        &self,
        _registration: &RuntimeSessionRegistrationWitness,
        _agent: SessionAgentBinding,
    ) -> Option<Arc<dyn ConfiguredMcpAttachmentAuthority>> {
        Some(Arc::new(AllowConfigured))
    }
}

fn principal(kind: PrincipalKind, name: &str) -> PrincipalRef {
    PrincipalRef::in_domain(
        kind,
        name,
        meerkat_core::TrustDomainId::new("meta-mcp-surface").expect("trust domain"),
    )
    .expect("principal")
}

/// The host's governed configuration: its own grant owner, refusing policy
/// owners and ingress, and every configured server allowed.
fn governed_configuration() -> NativeGrantWorkConfiguration {
    let grants = Arc::new(
        LocalGrantAuthority::new(
            LocalGrantConfiguration {
                root: principal(PrincipalKind::ServiceAccount, "grant-owner"),
                namespace: EvidenceId::new("meta-mcp-surface-grants").expect("namespace"),
                generation: 1,
            },
            LocalAuthorizationPublication::new(),
            Arc::new(HostAuthorizationClock),
        )
        .expect("host grant owner"),
    );
    let ingress: Arc<NativeIngressCheck> =
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
        configured_mcp_attachment: Some(Arc::new(AllowEveryConfiguredServer)),
    }
}

/// The host's application policy owner: one provider whose policy allows
/// every tool.
struct HostPolicy {
    id: meerkat_core::PolicyProviderId,
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
        Ok(Arc::new(AllowsEverything))
    }
}

fn host_registry() -> Arc<meerkat_core::ToolConsequencePolicyRegistry> {
    Arc::new(
        meerkat_core::ToolConsequencePolicyRegistry::new(
            vec![Arc::new(HostPolicy {
                id: meerkat_core::PolicyProviderId::new("host").expect("provider id"),
            })],
            meerkat_core::PolicyEvaluationSupervisorConfig::default(),
            None,
        )
        .expect("host policy registry"),
    )
}

fn destination(
    identity: &meerkat_mob::MemberRuntimeBuildIdentity,
) -> meerkat_mob::MemberDestinationSelection {
    let owner = principal(
        PrincipalKind::SharedAgent,
        identity.member().member.as_str(),
    );
    meerkat_mob::MemberDestinationSelection {
        logical_owner: owner.clone(),
        context: identity.session_document().clone(),
        context_generation: 1,
        audience: meerkat_mob::AudienceRef::Principal { principal: owner },
    }
}

/// One tool of a prepared surface as `<name>=<kind>:<source>`, or
/// `<name>=none` without provenance.
fn surface_tool(tool: &meerkat_core::ToolIdentity) -> String {
    match &tool.provenance {
        Some(provenance) => format!(
            "{}={:?}:{}",
            tool.name,
            provenance.kind,
            provenance.source_id.as_str()
        ),
        None => format!("{}=none", tool.name),
    }
}

/// Selects each member's own session document as its destination and the
/// host's policy for its prepared surface, and records that surface: one
/// line per construction, `surface <member> <tool> <tool> ...`, kept and,
/// with a record file, appended to it.
#[derive(Default)]
struct SurfaceRecordingHook {
    record: Option<PathBuf>,
    surfaces: Mutex<Vec<String>>,
}

impl SurfaceRecordingHook {
    fn surfaces(&self) -> Vec<String> {
        self.surfaces.lock().unwrap().clone()
    }
}

#[async_trait::async_trait]
impl meerkat_mob::MemberRuntimeRegistrationHook for SurfaceRecordingHook {
    async fn select_destination(
        &self,
        identity: &meerkat_mob::MemberRuntimeBuildIdentity,
    ) -> Result<meerkat_mob::MemberDestinationSelection, meerkat_mob::MobError> {
        Ok(destination(identity))
    }

    async fn expected_destination(
        &self,
        identity: &meerkat_mob::MemberRuntimeBuildIdentity,
    ) -> Result<meerkat_mob::MemberDestinationSelection, meerkat_mob::MobError> {
        Ok(destination(identity))
    }

    async fn verify_member_surface(
        &self,
        _identity: &meerkat_mob::MemberRuntimeBuildIdentity,
        _surface: &[meerkat_core::ToolDef],
    ) -> Result<(), meerkat_mob::MobError> {
        Ok(())
    }

    async fn prepare_member_surface(
        &self,
        identity: &meerkat_mob::MemberRuntimeBuildIdentity,
        surface: &meerkat_core::PreparedToolSurface,
    ) -> Result<meerkat_core::ToolSurfaceSelection, meerkat_mob::MobError> {
        let tools: Vec<String> = surface.tools.iter().map(surface_tool).collect();
        let line = format!("surface {} {}", identity.member().member, tools.join(" "));
        if let Some(record) = &self.record {
            append(record, &line);
        }
        self.surfaces.lock().unwrap().push(line);
        Ok(meerkat_core::ToolSurfaceSelection {
            binding: meerkat_core::ApplicationToolPolicyBinding::Provider {
                provider_id: meerkat_core::PolicyProviderId::new("host").expect("provider id"),
                policy_id: meerkat_core::PolicyId::new("member-tools").expect("policy id"),
            },
            compiled_against: surface.clone(),
        })
    }
}

fn append(record: &Path, line: &str) {
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(record)
        .expect("record");
    writeln!(file, "{line}").expect("append");
}

/// The tools of the recorded surface line `line`.
fn tools_of(line: &str) -> Vec<&str> {
    line.split(' ').skip(2).collect()
}

/// A gateway host that governs its members with [`governed_configuration`]
/// and decides each member's surface through a [`SurfaceRecordingHook`]
/// writing to its record file.
struct SurfaceHost {
    record: PathBuf,
}

#[async_trait::async_trait]
impl GatewayHostComposition for SurfaceHost {
    async fn prepare_bootstrap(
        &self,
        spec: meerkat_meta::MobBootstrapSpec,
    ) -> Result<meerkat_meta::MobBootstrapSpec, GatewayHostError> {
        Ok(spec
            .with_member_runtime_registration_hook(Arc::new(SurfaceRecordingHook {
                record: Some(self.record.clone()),
                surfaces: Mutex::new(Vec::new()),
            }))
            .with_tool_consequence_policy_registry(host_registry()))
    }

    fn native_grant_work_configuration(
        &self,
    ) -> Result<Option<NativeGrantWorkConfiguration>, GatewayHostError> {
        Ok(Some(governed_configuration()))
    }

    async fn bind_runtime(&self, _runtime: &mut UnifiedRuntime) -> Result<(), GatewayHostError> {
        Ok(())
    }

    fn publish_bootstrap_ready(
        &self,
        publication: GatewayBootstrapReady<'_>,
    ) -> Result<(), GatewayHostError> {
        publication.publish();
        Ok(())
    }

    fn begin_close(&self, _reason: GatewayHostCloseReason) -> Result<(), GatewayHostError> {
        Ok(())
    }

    async fn join_close(&self) -> Result<(), GatewayHostError> {
        Ok(())
    }

    async fn wait_closed(&self) {
        std::future::pending::<()>().await;
    }
}

/// On the composed stdio gateway, the server a member's profile configures
/// is part of the surface its host decides on: the member's one construction
/// is prepared after the server connected, so its tool is there, with the
/// server's MCP provenance. A surface captured while the server was still
/// connecting has no MCP tool, and nothing the host issues for that surface
/// covers the server. A separate gateway process over real stdio.
#[test]
fn a_profile_mcp_server_reaches_the_prepared_surface_on_the_stdio_gateway() {
    if std::env::var_os(HOST_PROCESS).is_some() {
        let host = Arc::new(SurfaceHost {
            record: std::env::var_os(RECORD).expect("record path").into(),
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
    let dir = tempfile::tempdir().unwrap();
    let record = dir.path().join("record");
    let stderr_path = dir.path().join("stderr");
    let listing = listing_server(dir.path());
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", GATEWAY_TEST, "--nocapture"])
        .env(HOST_PROCESS, "1")
        .env(RECORD, &record)
        .env("OPENAI_API_KEY", FIXTURE_OPENAI_KEY)
        .env("XDG_STATE_HOME", dir.path().join("xdg-state"))
        .current_dir(dir.path())
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::fs::File::create(&stderr_path).expect("stderr file"))
        .spawn()
        .expect("gateway host process");
    let stdout = child.stdout.take().expect("stdout");
    let (tx, lines) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { break };
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    let mut stdin = child.stdin.take().expect("stdin");
    let mut send = |value: Value| {
        writeln!(stdin, "{value}").expect("write");
        stdin.flush().expect("flush");
    };
    send(json!({
        "jsonrpc": "2.0",
        "id": "init",
        "method": "meta/init",
        "params": {
            "init_protocol": "accepted_then_settled",
            "init_id": "member-mcp-surface",
            "mob_config": mob_config("member-mcp-surface-gateway", &listing, false),
            "persistent_state": dir.path().join("state"),
        },
    }));

    let started = Instant::now();
    let mut seen = Vec::new();
    let mut spawned = None;
    while spawned.is_none() {
        let line = match lines.recv_timeout(BACKSTOP.saturating_sub(started.elapsed())) {
            Ok(line) => line,
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                eprint!(
                    "{}",
                    std::fs::read_to_string(&stderr_path).unwrap_or_default()
                );
                panic!("the run did not finish ({error:?}); seen {seen:?}");
            }
        };
        // The test harness of the host process may print before the first
        // message on the same line.
        let Some(message) = line
            .find('{')
            .and_then(|start| serde_json::from_str::<Value>(line[start..].trim()).ok())
        else {
            continue;
        };
        if message.get("method").and_then(Value::as_str) == Some("meta/init_settled") {
            assert_eq!(
                message["params"]["outcome"], "ready",
                "the governed gateway settles ready: {message}"
            );
            send(json!({
                "jsonrpc": "2.0",
                "id": "spawn",
                "method": "meta/spawn_member",
                "params": { "profile": "worker", "meerkat_id": "worker-1" },
            }));
        } else if message.get("method").is_none() && message["id"] == "spawn" {
            spawned = Some(message.clone());
        }
        seen.push(message);
    }
    drop(stdin);
    let deadline = Instant::now() + BACKSTOP;
    while child.try_wait().unwrap().is_none() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    let exited = child.try_wait().unwrap().is_some();
    if !exited {
        let _ = child.kill();
    }
    let _ = child.wait();
    // The host process's log rides this test's own output.
    eprint!(
        "{}",
        std::fs::read_to_string(&stderr_path).unwrap_or_default()
    );
    let spawned = spawned.expect("the spawn answered");
    assert!(
        spawned.get("result").is_some(),
        "the member spawns: {spawned}"
    );
    let recorded = std::fs::read_to_string(&record).unwrap_or_default();
    let surfaces: Vec<&str> = recorded
        .lines()
        .filter(|line| line.starts_with("surface worker-1 "))
        .collect();
    assert_eq!(
        surfaces.len(),
        1,
        "the member's one construction is decided by its host: {recorded}"
    );
    let lookup = format!("{LOOKUP}=Mcp:{LISTING}");
    assert!(
        tools_of(surfaces[0]).contains(&lookup.as_str()),
        "the prepared surface carries the profile server's tool with its MCP provenance: {}",
        surfaces[0]
    );
    assert!(exited, "the gateway ends after its run");
}

/// One member of a library-built runtime (persistent or ephemeral) whose
/// profile configures the listing server and one that exits before its
/// handshake: it still spawns, once the failed server has failed. Its
/// session holds the typed setup-failure notice for that server, and the
/// surface its host decided on carries the healthy server's tool and nothing
/// of the failed one.
async fn assert_a_failed_server_leaves_the_member_spawned(persistent: bool) {
    let mode = if persistent {
        "persistent"
    } else {
        "ephemeral"
    };
    let dir = tempfile::tempdir().unwrap();
    let listing = listing_server(dir.path());
    let hook = Arc::new(SurfaceRecordingHook::default());
    let mut config = meerkat::Config::default();
    config.realm.insert(
        meerkat_core::connection::GLOBAL_REALM_SLUG.to_string(),
        meerkat_core::RealmConfigSection::from_inline_api_keys(&[("openai", FIXTURE_OPENAI_KEY)]),
    );
    let definition = meerkat_mob::MobDefinition::from_toml(&mob_config(
        &format!("member-mcp-surface-library-{mode}"),
        &listing,
        true,
    ))
    .expect("mob definition");
    let mut builder = UnifiedRuntime::builder()
        .definition(definition)
        .meerkat_config(config)
        .native_grant_work_configuration(governed_configuration())
        .member_runtime_registration_hook(hook.clone())
        .tool_consequence_policy_registry(host_registry());
    if persistent {
        builder = builder.persistent_state(dir.path().join("state"));
    }
    let runtime = builder.build().await.expect("build");
    runtime
        .mob_handle()
        .ensure_member(meerkat_mob::SpawnMemberSpec::new(
            meerkat_mob::ProfileName::from("worker"),
            meerkat_mob::ids::AgentIdentity::from("worker-1"),
        ))
        .await
        .unwrap_or_else(|error| {
            panic!("{mode}: a member whose server fails still spawns: {error}")
        });

    let surfaces = hook.surfaces();
    assert_eq!(
        surfaces.len(),
        1,
        "{mode}: the member's one construction is decided by its host: {surfaces:?}"
    );
    let tools = tools_of(&surfaces[0]);
    let lookup = format!("{LOOKUP}=Mcp:{LISTING}");
    assert!(
        tools.contains(&lookup.as_str()),
        "{mode}: the healthy server's tool is on the prepared surface: {}",
        surfaces[0]
    );
    let broken = format!(":{BROKEN}");
    assert!(
        !tools.iter().any(|tool| tool.ends_with(&broken)),
        "{mode}: nothing of the failed server is on the prepared surface: {}",
        surfaces[0]
    );

    let service = runtime
        .mob_runtime()
        .session_service()
        .expect("the runtime-backed spec has a session service");
    let sessions = service
        .list(meerkat_core::service::SessionQuery::default())
        .await
        .expect("sessions");
    assert_eq!(sessions.len(), 1, "{mode}: the member's one session");
    let history = service
        .read_history(
            &sessions[0].session_id,
            meerkat_core::service::SessionHistoryQuery::default(),
        )
        .await
        .expect("the member's history");
    let told = history.messages.iter().any(|message| {
        let meerkat_core::types::Message::SystemNotice(notice) = message else {
            return false;
        };
        notice.kind == meerkat_core::types::SystemNoticeKind::Mcp
            && notice.blocks.iter().any(|block| {
                matches!(
                    block,
                    meerkat_core::types::SystemNoticeBlock::Mcp {
                        server_id: Some(server),
                        phase: Some(meerkat_core::ExternalToolDeltaPhase::Failed),
                        ..
                    } if server == BROKEN
                )
            })
    });
    assert!(
        told,
        "{mode}: the member's session holds the typed setup-failure notice for {BROKEN}: {:?}",
        history.messages
    );
    let _ = runtime.shutdown().await;
}

/// On the persistent library builder, a member whose configured server fails
/// still spawns, told of the failure and without that server's tools.
#[tokio::test(flavor = "multi_thread")]
async fn a_member_whose_mcp_server_fails_spawns_with_its_notice_and_without_its_tools() {
    assert_a_failed_server_leaves_the_member_spawned(true).await;
}

/// The same on the ephemeral library builder.
#[tokio::test(flavor = "multi_thread")]
async fn an_ephemeral_member_whose_mcp_server_fails_spawns_with_its_notice_and_without_its_tools() {
    assert_a_failed_server_leaves_the_member_spawned(false).await;
}

/// How long the slow listing server reads nothing after it starts.
const SLOW_START: Duration = Duration::from_secs(3);

/// [`LISTING_SERVER`] that reads nothing for [`SLOW_START`] first, so its
/// handshake completes only then.
fn slow_listing_server(dir: &Path) -> PathBuf {
    let path = dir.join("slow-listing.sh");
    std::fs::write(
        &path,
        format!("sleep {}\n{LISTING_SERVER}", SLOW_START.as_secs()),
    )
    .expect("write the slow listing server");
    path
}

/// One request the model was sent: when it arrived, its tools as
/// `<name>=<kind>:<source>`, and the MCP servers its typed setup-failure
/// notices name.
#[derive(Clone, Debug)]
struct CapturedRequest {
    at: Instant,
    tools: Vec<String>,
    failed_mcp_servers: Vec<String>,
}

/// Records each request the model is sent.
#[derive(Clone, Default)]
struct CaptureClient {
    requests: Arc<Mutex<Vec<CapturedRequest>>>,
    notify: Arc<tokio::sync::Notify>,
}

impl CaptureClient {
    async fn first_request(&self) -> CapturedRequest {
        tokio::time::timeout(BACKSTOP, async {
            loop {
                let notified = self.notify.notified();
                if let Some(first) = self.requests.lock().unwrap().first() {
                    return first.clone();
                }
                notified.await;
            }
        })
        .await
        .expect("the member's first turn reaches the model")
    }
}

impl meerkat_client::LlmClient for CaptureClient {
    fn project_replay_messages(
        &self,
        messages: &[meerkat_core::Message],
    ) -> Result<Vec<meerkat_core::Message>, meerkat_client::LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(
        &'a self,
        request: &'a meerkat_client::LlmRequest,
    ) -> std::pin::Pin<
        Box<
            dyn futures::Stream<Item = Result<meerkat_client::LlmEvent, meerkat_client::LlmError>>
                + Send
                + 'a,
        >,
    > {
        let tools = request
            .tools
            .iter()
            .map(|tool| match &tool.provenance {
                Some(provenance) => format!(
                    "{}={:?}:{}",
                    tool.name,
                    provenance.kind,
                    provenance.source_id.as_str()
                ),
                None => format!("{}=none", tool.name),
            })
            .collect();
        let failed_mcp_servers = request
            .messages
            .iter()
            .filter_map(|message| match message {
                meerkat_core::types::Message::SystemNotice(notice)
                    if notice.kind == meerkat_core::types::SystemNoticeKind::Mcp =>
                {
                    Some(notice.blocks.iter().filter_map(|block| match block {
                        meerkat_core::types::SystemNoticeBlock::Mcp {
                            server_id: Some(server),
                            phase: Some(meerkat_core::ExternalToolDeltaPhase::Failed),
                            ..
                        } => Some(server.clone()),
                        _ => None,
                    }))
                }
                _ => None,
            })
            .flatten()
            .collect();
        self.requests.lock().unwrap().push(CapturedRequest {
            at: Instant::now(),
            tools,
            failed_mcp_servers,
        });
        self.notify.notify_waiters();
        let [usage, done] = llm_usage::usage_then_done(
            request,
            meerkat::Provider::OpenAI,
            meerkat_core::types::StopReason::EndTurn,
        );
        Box::pin(async_stream::stream! {
            yield Ok(meerkat_client::LlmEvent::TextDelta { delta: "ok".to_string(), meta: None });
            yield Ok(usage);
            yield Ok(done);
        })
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

/// A member of a spec from the plain public constructor
/// (`MobBootstrapSpec::ephemeral`) is built once its slow server has
/// connected, so the first request its turn sends the model already offers
/// that server's tool. Built while the server was still starting, the first
/// request would not.
#[tokio::test(flavor = "multi_thread")]
async fn a_plain_ephemeral_spec_builds_its_member_after_its_slow_server_connects() {
    let dir = tempfile::tempdir().unwrap();
    let server = slow_listing_server(dir.path());
    let definition = meerkat_mob::MobDefinition::from_toml(&format!(
        r#"
[mob]
id = "member-mcp-surface-plain"

[profiles.worker]
model = "gpt-5.5"
runtime_mode = "autonomous_host"
external_addressable = true

[profiles.worker.tools]
comms = true

[[profiles.worker.tools.mcp_servers]]
name = "{LISTING}"
command = "/bin/sh"
args = [{server:?}]
"#,
        server = server.display().to_string(),
    ))
    .expect("mob definition");
    let client = CaptureClient::default();
    let spec = meerkat_meta::MobBootstrapSpec::ephemeral(
        definition,
        meerkat_mob::MobStorage::in_memory(),
        dir.path().join("store"),
        4,
        None,
    )
    .expect("plain ephemeral spec")
    .with_options(meerkat_meta::MobBootstrapOptions {
        allow_ephemeral_sessions: true,
        notify_orchestrator_on_resume: false,
        default_llm_client: Some(Arc::new(client.clone())),
    });
    let runtime = meerkat_meta::MobRuntime::bootstrap(spec)
        .await
        .expect("bootstrap");
    Box::pin(
        runtime
            .handle()
            .spawn_spec(meerkat_mob::SpawnMemberSpec::host_root(
                "worker",
                meerkat_mob::AgentIdentity::from("worker-1"),
            )),
    )
    .await
    .expect("spawn");
    meerkat_meta::send_message_on_mob(&runtime.handle(), "worker-1", "hello".to_string())
        .await
        .expect("send");
    let first = client.first_request().await;
    let lookup = format!("{LOOKUP}=Mcp:{LISTING}");
    assert!(
        first.tools.contains(&lookup),
        "the first request offers the slow server's tool: {first:?}"
    );
    let _ = runtime.handle().shutdown().await;
}

/// A server that never answers its handshake.
const SILENT: &str = "silent";
/// Its own connect timeout.
const SILENT_CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
/// The close of a connect that never completed its handshake: at most this
/// EOF grace after the deadline, then SIGTERM, which this server obeys.
const UNSETTLED_CONNECT_EOF_GRACE: Duration = Duration::from_millis(250);
/// What a member's build and the start of its first turn take beyond waiting
/// for its servers, on a loaded host. The bound it sets stays far below
/// Core's 10 s default connect timeout, so a first request inside it was
/// waited out by the server's own connect timeout and by nothing longer.
const BUILD_AND_TURN_ALLOWANCE: Duration = Duration::from_secs(5);

/// A server that never answers its handshake delays a member's first turn
/// only up to its own connect timeout, plus the at most 0.25 s EOF grace its
/// close gets: the first request reaches the model no sooner than the
/// timeout and long before anything longer would end the wait. It carries
/// the server's typed setup-failure notice and none of its tools.
#[tokio::test(flavor = "multi_thread")]
async fn a_silent_mcp_server_delays_the_first_turn_only_up_to_its_connect_timeout() {
    let dir = tempfile::tempdir().unwrap();
    let definition = meerkat_mob::MobDefinition::from_toml(&format!(
        r#"
[mob]
id = "member-mcp-surface-silent"

[profiles.worker]
model = "gpt-5.5"
runtime_mode = "autonomous_host"
external_addressable = true

[profiles.worker.tools]
comms = true

[[profiles.worker.tools.mcp_servers]]
name = "{SILENT}"
command = "/bin/sh"
args = ["-c", "exec sleep 30"]
connect_timeout_secs = {timeout}
"#,
        timeout = SILENT_CONNECT_TIMEOUT.as_secs(),
    ))
    .expect("mob definition");
    let client = CaptureClient::default();
    let spec = meerkat_meta::MobBootstrapSpec::ephemeral(
        definition,
        meerkat_mob::MobStorage::in_memory(),
        dir.path().join("store"),
        4,
        None,
    )
    .expect("plain ephemeral spec")
    .with_options(meerkat_meta::MobBootstrapOptions {
        allow_ephemeral_sessions: true,
        notify_orchestrator_on_resume: false,
        default_llm_client: Some(Arc::new(client.clone())),
    });
    let runtime = meerkat_meta::MobRuntime::bootstrap(spec)
        .await
        .expect("bootstrap");
    let started = Instant::now();
    Box::pin(
        runtime
            .handle()
            .spawn_spec(meerkat_mob::SpawnMemberSpec::host_root(
                "worker",
                meerkat_mob::AgentIdentity::from("worker-1"),
            )),
    )
    .await
    .expect("a member whose server never answers still spawns");
    meerkat_meta::send_message_on_mob(&runtime.handle(), "worker-1", "hello".to_string())
        .await
        .expect("send");
    let first = client.first_request().await;
    let waited = first.at.duration_since(started);
    assert!(
        waited >= SILENT_CONNECT_TIMEOUT,
        "the first turn waited for the server's connect: {waited:?}"
    );
    assert!(
        waited < SILENT_CONNECT_TIMEOUT + UNSETTLED_CONNECT_EOF_GRACE + BUILD_AND_TURN_ALLOWANCE,
        "the first turn waited only up to the server's connect timeout: {waited:?}"
    );
    assert!(
        first
            .failed_mcp_servers
            .iter()
            .any(|server| server == SILENT),
        "the first request carries the server's typed setup-failure notice: {first:?}"
    );
    let silent = format!(":{SILENT}");
    assert!(
        !first.tools.iter().any(|tool| tool.ends_with(&silent)),
        "the first request offers none of the server's tools: {first:?}"
    );
    let _ = runtime.handle().shutdown().await;
}
