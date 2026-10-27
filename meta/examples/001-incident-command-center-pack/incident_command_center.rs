#![allow(dead_code)]

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use async_trait::async_trait;
use meerkat::AgentToolDispatcher;
use meerkat_client::LlmClient;
use meerkat_core::ToolCategoryOverride;
use meerkat_core::error::ToolError;
use meerkat_core::service::{CreateSessionRequest, SessionBuildOptions};
use meerkat_core::{
    ToolCallView, ToolDef, ToolDispatchOutcome, ToolProvenance, ToolResult, ToolSourceKind,
};
// meerkat 0.7: the MeerkatId alias was deleted; member ids are AgentIdentity.
use meerkat_mob::ids::AgentIdentity as MeerkatId;
use meerkat_mob::{MobDefinition, ProfileName, SpawnMemberSpec};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::time::sleep;

use meerkat_meta::decisions::{AuthPolicy, BigQueryNaming, ConsolePolicy, RuntimeOpsPolicy};
use meerkat_meta::mob_handle_runtime::{SessionCreatedContext, SessionHook};
use meerkat_meta::runtime::{
    DeliverySendRequest, GatingDecideRequest, GatingDecision, GatingEvaluateRequest,
    GatingPendingEntry, GatingRiskTier, RuntimeRoute,
};
use meerkat_meta::unified_runtime::{DesiredPeerEdge, EdgeDiscovery, UnifiedRuntime};
use meerkat_meta::{
    AgentMemoryConfig, DiscoverySpec, MetaConfig, ModuleConfig, PreSpawnData, RestartPolicy,
    RuntimeDecisionInputs, RuntimeDecisionState, SqliteAgentMemoryStore, TopologyControlPolicy,
    TrustedOidcRuntimeConfig, build_runtime_decision_state,
};

const BOUNDARY_ENV_KEY: &str = "META_MODULE_BOUNDARY";
const BOUNDARY_ENV_VALUE_MCP: &str = "mcp";

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct IncidentScenario {
    pub scenario_id: String,
    pub namespace: String,
    pub listen_addr: String,
    pub approver_id: String,
    #[serde(default)]
    pub identities: Vec<IncidentIdentity>,
    #[serde(default)]
    pub links: Vec<IncidentLink>,
    /// Optional operator topology control. The Meta default is disabled;
    /// this stock example opts in explicitly in scenario.yaml.
    #[serde(default)]
    pub topology_control: TopologyControlPolicy,
    #[serde(default)]
    pub routes: Vec<IncidentRouteSeed>,
    #[serde(default)]
    pub deliveries: Vec<IncidentDeliverySeed>,
    #[serde(default)]
    pub gating: Vec<IncidentGatingSeed>,
    #[serde(default)]
    pub smoke: IncidentSmokeExpectations,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct IncidentIdentity {
    pub identity: String,
    pub profile: String,
    pub display_name: String,
    pub group: String,
    #[serde(default = "default_addressable")]
    pub addressable: bool,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct IncidentLink {
    pub from: String,
    pub to: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct IncidentRouteSeed {
    pub route_key: String,
    pub recipient: String,
    #[serde(default)]
    pub channel: Option<String>,
    pub sink: String,
    #[serde(default = "default_delivery_target_module")]
    pub target_module: String,
    #[serde(default)]
    pub retry_max: Option<u32>,
    #[serde(default)]
    pub backoff_ms: Option<u64>,
    #[serde(default)]
    pub rate_limit_per_minute: Option<u32>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct IncidentDeliverySeed {
    pub recipient: String,
    pub channel: String,
    pub payload: Value,
    pub idempotency_key: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct IncidentGatingSeed {
    pub action: String,
    pub actor_id: String,
    pub risk_tier: String,
    pub requested_approver: String,
    pub approval_recipient: String,
    pub approval_channel: String,
    #[serde(default)]
    pub approval_timeout_ms: Option<u64>,
    #[serde(default)]
    pub rationale: Option<String>,
    #[serde(default)]
    pub entity: Option<String>,
    #[serde(default)]
    pub topic: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct IncidentSmokeExpectations {
    #[serde(default)]
    pub watched_identities: Vec<String>,
    #[serde(default)]
    pub degraded_identities: Vec<String>,
    #[serde(default)]
    pub critical_identities: Vec<String>,
    #[serde(default)]
    pub prompts: IncidentSmokePrompts,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct IncidentSmokePrompts {
    #[serde(default)]
    pub tool_sweep: String,
    #[serde(default)]
    pub merchant_status: String,
    #[serde(default)]
    pub alpha_follow_up: String,
    #[serde(default)]
    pub bravo_follow_up: String,
}

#[derive(Debug, Clone, JsonSchema, Deserialize)]
struct InspectServiceArgs {
    service: String,
}

#[derive(Debug, Clone, JsonSchema, Deserialize)]
struct AnalyzeImpactArgs {
    cohort: String,
}

#[derive(Clone)]
pub struct IncidentRuntimeBundle {
    pub scenario: IncidentScenario,
    pub runtime: Arc<UnifiedRuntime>,
    pub decisions: RuntimeDecisionState,
}

pub fn scenario_path() -> Result<PathBuf> {
    Ok(PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .context("workspace root")?
        .join("examples")
        .join("001-incident-command-center-pack")
        .join("scenario.yaml"))
}

pub fn load_scenario(path: &Path) -> Result<IncidentScenario> {
    let raw = fs::read_to_string(path)
        .with_context(|| format!("failed to read scenario at {}", path.display()))?;
    // YAML 1.2 booleans, `<<` an ordinary key, duplicate keys and unknown
    // tags refused.
    let mut options = serde_saphyr::Options::default();
    options.strict_booleans = true;
    options.merge_keys = serde_saphyr::MergeKeyPolicy::AsOrdinary;
    options.duplicate_keys = serde_saphyr::DuplicateKeyPolicy::Error;
    options.reject_unsupported_tags = true;
    let scenario = serde_saphyr::from_str_with_options::<IncidentScenario>(&raw, options)
        .with_context(|| format!("failed to parse scenario at {}", path.display()))?;
    Ok(scenario)
}

pub async fn build_runtime_bundle(path: &Path) -> Result<IncidentRuntimeBundle> {
    let scenario = load_scenario(path)?;
    Box::pin(build_runtime_bundle_with_client(&scenario, None)).await
}

pub async fn build_runtime_bundle_with_default_client(
    path: &Path,
    default_llm_client: Arc<dyn LlmClient>,
) -> Result<IncidentRuntimeBundle> {
    let scenario = load_scenario(path)?;
    Box::pin(build_runtime_bundle_with_client(
        &scenario,
        Some(default_llm_client),
    ))
    .await
}

async fn build_runtime_bundle_with_client(
    scenario: &IncidentScenario,
    default_llm_client: Option<Arc<dyn LlmClient>>,
) -> Result<IncidentRuntimeBundle> {
    let definition = incident_definition()?;
    // Classic-mob agent memory (no roster / IdentityRuntime): the recorder
    // `memory` tool + build-time injection attach per member via the spawn
    // customizer, and the bundled SQLite store auto-wires the Memory panel.
    // Records accrue organically as the incident agents record durable facts
    // (see the memory guidance in the commander/scribe roles).
    let memory_dir = std::env::var("INCIDENT_COMMAND_CENTER_MEMORY_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir().join("incident-command-center-memory"));
    std::fs::create_dir_all(&memory_dir).ok();
    let memory_store =
        SqliteAgentMemoryStore::open(&memory_dir).context("open incident agent-memory store")?;

    let mut builder = UnifiedRuntime::builder()
        .definition(definition)
        .session_hook(Arc::new(IncidentSessionHook))
        .module_config(example_module_config(scenario)?)
        .edge_discovery(ScenarioEdgeDiscovery::new(scenario.links.clone()))
        .agent_memory(
            Arc::new(memory_store),
            AgentMemoryConfig {
                recorder_tool: true,
                ..AgentMemoryConfig::default()
            },
        )
        .timeout(Duration::from_mins(1));
    builder = builder
        .topology_control(scenario.topology_control.clone())
        .context("configure incident topology control")?;
    // Editable topology is intentionally a durable control plane. Keep the
    // stock example valid even when it is embedded by tests or launched
    // directly without the browser-smoke environment wrapper. Operators can
    // still choose an explicit restart-stable location through the env var.
    let state_dir = std::env::var("INCIDENT_COMMAND_CENTER_STATE_DIR")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(default_incident_state_dir);
    builder = builder.persistent_state(state_dir);
    if let Some(client) = default_llm_client.or_else(default_incident_llm_client_from_env) {
        builder = builder.default_llm_client(client);
    }
    let runtime = Box::pin(builder.build())
        .await
        .context("build incident runtime")?;

    seed_runtime(&runtime, scenario).await?;

    let decisions = build_runtime_decision_state(RuntimeDecisionInputs {
        bigquery: BigQueryNaming {
            dataset: format!("{}_dataset", scenario.namespace.replace('-', "_")),
            table: "console_events".to_string(),
        },
        trusted_meta_toml: trusted_modules_toml(),
        auth: AuthPolicy::default(),
        trusted_oidc: trusted_oidc(),
        console: ConsolePolicy {
            require_app_auth: false,
            ..ConsolePolicy::default()
        },
        ops: RuntimeOpsPolicy::default(),
        release_metadata_json: include_str!(
            "../../crates/meerkat-meta/assets/release-targets.json"
        )
        .to_string(),
    })
    .map_err(|err| anyhow!("failed to build incident console decisions: {err:?}"))?;

    Ok(IncidentRuntimeBundle {
        scenario: scenario.clone(),
        runtime: Arc::new(runtime),
        decisions,
    })
}

fn default_incident_llm_client_from_env() -> Option<Arc<dyn LlmClient>> {
    let api_key = std::env::var("OPENAI_API_KEY")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())?;
    let base_url =
        std::env::var("OPENAI_BASE_URL").unwrap_or_else(|_| "https://api.openai.com".to_string());
    Some(Arc::new(
        meerkat_client::OpenAiClient::new_with_optional_api_key_and_base_url(
            Some(api_key),
            base_url,
        ),
    ))
}

pub async fn seed_runtime(runtime: &UnifiedRuntime, scenario: &IncidentScenario) -> Result<()> {
    runtime
        .reconcile_modules(
            vec!["router".to_string(), "delivery".to_string()],
            incident_module_reconcile_timeout(),
        )
        .await
        .context("reconcile incident modules")?;

    runtime
        .reconcile(
            scenario
                .identities
                .iter()
                .map(identity_to_spawn_spec)
                .collect::<Vec<_>>(),
        )
        .await
        .context("reconcile incident roster")?;
    wait_for_scenario_wiring(runtime, &scenario.links).await?;

    for route in &scenario.routes {
        runtime
            .add_runtime_route(RuntimeRoute {
                route_key: route.route_key.clone(),
                recipient: route.recipient.clone(),
                channel: route.channel.clone(),
                sink: route.sink.clone(),
                target_module: route.target_module.clone(),
                retry_max: route.retry_max,
                backoff_ms: route.backoff_ms,
                rate_limit_per_minute: route.rate_limit_per_minute,
            })
            .await
            .with_context(|| format!("add route {}", route.route_key))?;
    }

    for delivery in &scenario.deliveries {
        let resolution = runtime
            .resolve_routing(meerkat_meta::runtime::RoutingResolveRequest {
                recipient: delivery.recipient.clone(),
                channel: Some(delivery.channel.clone()),
                retry_max: None,
                backoff_ms: None,
                rate_limit_per_minute: None,
            })
            .await
            .with_context(|| format!("resolve delivery recipient {}", delivery.recipient))?;
        runtime
            .send_delivery(DeliverySendRequest {
                resolution,
                payload: delivery.payload.clone(),
                idempotency_key: Some(delivery.idempotency_key.clone()),
            })
            .await
            .with_context(|| format!("seed delivery {}", delivery.idempotency_key))?;
    }

    for gating in &scenario.gating {
        runtime
            .evaluate_gating_action(GatingEvaluateRequest {
                action: gating.action.clone(),
                actor_id: gating.actor_id.clone(),
                risk_tier: parse_risk_tier(&gating.risk_tier)?,
                rationale: gating.rationale.clone(),
                requested_approver: Some(gating.requested_approver.clone()),
                approval_recipient: Some(gating.approval_recipient.clone()),
                approval_channel: Some(gating.approval_channel.clone()),
                approval_timeout_ms: gating.approval_timeout_ms,
                entity: gating.entity.clone(),
                topic: gating.topic.clone(),
            })
            .await;
    }

    Ok(())
}

fn incident_module_reconcile_timeout() -> Duration {
    let configured = std::env::var("INCIDENT_COMMAND_CENTER_MODULE_RECONCILE_TIMEOUT_SECS")
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .filter(|seconds| *seconds > 0);
    Duration::from_secs(configured.unwrap_or_else(|| {
        // The deterministic topology lane may need to compile its generated
        // module binaries on first boot. The live example keeps its tight
        // operational timeout; the offline acceptance gives that cold build a
        // bounded readiness window instead of failing after compilation.
        if incident_env_flag("INCIDENT_COMMAND_CENTER_OFFLINE") {
            600
        } else {
            5
        }
    }))
}

fn incident_env_flag(key: &str) -> bool {
    std::env::var(key).is_ok_and(|value| {
        matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    })
}

async fn wait_for_scenario_wiring(runtime: &UnifiedRuntime, links: &[IncidentLink]) -> Result<()> {
    if links.is_empty() {
        return Ok(());
    }

    const MAX_EDGE_RECONCILE_ATTEMPTS: usize = 30;
    let mut last_missing = Vec::new();
    let mut last_report = None;
    for attempt in 0..MAX_EDGE_RECONCILE_ATTEMPTS {
        let report = runtime.reconcile_edges().await;
        let entries = runtime.mob_handle().list_members_including_retiring().await;
        let wired_to = entries
            .into_iter()
            .map(|entry| {
                (
                    entry.agent_identity.to_string(),
                    entry
                        .wired_to
                        .into_iter()
                        .map(|peer| peer.to_string())
                        .collect::<BTreeSet<_>>(),
                )
            })
            .collect::<BTreeMap<_, _>>();
        let suppressed = runtime
            .topology_runtime_handle()
            .query()
            .await
            .context("query topology intent while waiting for incident wiring")?
            .edges
            .into_iter()
            .filter(|edge| edge.suppressed)
            .map(|edge| canonical_incident_edge(&edge.edge.a.identity, &edge.edge.b.identity))
            .collect::<BTreeSet<_>>();
        last_missing = missing_scenario_links(links, &wired_to, &suppressed);
        if last_missing.is_empty() {
            return Ok(());
        }
        last_report = Some(report);
        if attempt + 1 < MAX_EDGE_RECONCILE_ATTEMPTS {
            sleep(Duration::from_millis(100)).await;
        }
    }

    bail!(
        "incident runtime wiring incomplete after retries; missing links: {}; last edge report: {:?}",
        last_missing.join(", "),
        last_report
    )
}

fn missing_scenario_links(
    links: &[IncidentLink],
    wired_to: &BTreeMap<String, BTreeSet<String>>,
    suppressed: &BTreeSet<(String, String)>,
) -> Vec<String> {
    links
        .iter()
        .filter(|link| !suppressed.contains(&canonical_incident_edge(&link.from, &link.to)))
        .filter_map(|link| {
            let from_has_to = wired_to
                .get(&link.from)
                .is_some_and(|peers| peers.contains(&link.to));
            let to_has_from = wired_to
                .get(&link.to)
                .is_some_and(|peers| peers.contains(&link.from));
            if from_has_to || to_has_from {
                None
            } else {
                Some(format!("{} -> {}", link.from, link.to))
            }
        })
        .collect()
}

fn canonical_incident_edge(left: &str, right: &str) -> (String, String) {
    if left < right {
        (left.to_string(), right.to_string())
    } else {
        (right.to_string(), left.to_string())
    }
}

pub async fn seed_escalation_chain(
    runtime: &UnifiedRuntime,
    pending_id: &str,
    approver_id: &str,
) -> Result<GatingPendingEntry> {
    runtime
        .set_gating_escalation_routes(meerkat_meta::GatingEscalationRoutes {
            default: Some("incident-commander".to_string()),
            ..meerkat_meta::GatingEscalationRoutes::default()
        })
        .await;
    let escalation = runtime
        .decide_gating_action(GatingDecideRequest {
            pending_id: pending_id.to_string(),
            approver_id: approver_id.to_string(),
            decision: GatingDecision::Escalate,
            reason: Some("incident_command_center_smoke".to_string()),
        })
        .await
        .context("escalate gating entry")?;
    let next_pending_id = escalation
        .next_pending_id
        .clone()
        .ok_or_else(|| anyhow!("expected escalation to create successor pending entry"))?;
    let pending = runtime.list_gating_pending().await;
    pending
        .into_iter()
        .find(|entry| entry.pending_id == next_pending_id)
        .ok_or_else(|| anyhow!("successor pending entry {next_pending_id} not found"))
}

fn default_addressable() -> bool {
    true
}

fn default_delivery_target_module() -> String {
    "delivery".to_string()
}

pub fn incident_model() -> String {
    std::env::var("RKAT_INCIDENT_MODEL").unwrap_or_else(|_| "gpt-5.5".to_string())
}

pub fn incident_image_model() -> String {
    std::env::var("RKAT_INCIDENT_IMAGE_MODEL").unwrap_or_else(|_| "gpt-image-2".to_string())
}

pub(crate) fn incident_definition() -> Result<MobDefinition> {
    let model = incident_model();
    let image_model = incident_image_model();
    MobDefinition::from_toml(&format!(
        r#"
[mob]
id = "incident-command-center"

[skills.comms_protocol]
source = "inline"
content = """
## Incident Comms Protocol

- Ignore mob.peer_added and mob.peer_retired lifecycle chatter. Do not reply to those notices.
- Use the `peers` tool before your first substantive peer message in a turn so you know who is reachable.
- Use `peer_request` when you need another teammate to answer a question or return facts to you.
- Use `peer_response` with `in_reply_to` when answering a peer request.
- When sending `peer_response`, put the actual answer in the `result` payload as plain fields like `summary`, `status_line`, or `facts`; do not send an empty result object.
- Use `peer_message` only for one-way updates or FYIs that do not require a reply.
- When responding to a teammate, send the answer back over comms. Do not keep the answer local-only.
- Keep peer messages short and factual: 1-3 sentences or tight bullets.
- If you learn a new material fact, notify scribe unless you are scribe.
- To forward a generated image from a `generate_image` result, send a comms block
  like `{{"type":"image_ref","source":"blob","blob_id":"sha256:...","media_type":"image/png"}}`.
  Use `source:"current_turn"` only for images attached by the operator in the current input.
"""

[skills.commander_role]
source = "inline"
content = """
You are the incident commander for the fictional CardinalPay payments outage.

TEAMMATES YOU SHOULD USE:
- payments-sre: live service health, mitigations, rollback safety
- api-investigator: root cause, blast radius, rollback reasoning
- merchant-comms: external and status-page wording
- merchant-success: VIP/customer-friendly phrasing
- scribe: timeline and established facts

HOW TO OPERATE:
- For any operator request about current status, customer impact, rollback, or publication readiness, use `peers` first and then send concise `peer_request` questions to at least two relevant teammates before you finalize your answer.
- Default coordination path for a status sweep: payments-sre + merchant-comms + scribe.
- Ask api-investigator whenever root cause or rollback confidence is part of the question.
- For visible one-off delegated investigation, use mob tools rather than only peer comms; spawned worker meerkats should appear in the shared console runtime.
- After a meaningful exchange, send a short factual note to scribe.
- When a teammate replies with `peer_response`, read the actual answer from the response payload fields such as `result.summary`, `result.status_line`, or `result.facts`. Do not treat a completed response as empty if those fields are present.
- Your final operator answer should be concise, operationally useful, and mention which teammates you consulted.
- Do not pretend a peer confirmed something if they have not replied.
- When the operator explicitly asks you to generate an image, call `generate_image` directly without consulting peers first. Include `provider: "openai"` and the configured image model (`model: "{image_model}"`) in the image request so the example uses the selected image route instead of the provider default. If asked why a model was used, say it came from the example's image-model configuration, not from a fixed policy preference.
- You have a `memory` tool for durable knowledge that should outlive this incident. When you establish or decide something durable — the affected service, the confirmed root cause, the chosen mitigation, an operator's stated preference — record it with `memory` (`action: "remember"`, a one-line `title`, a `description` of when it matters next time, the fact in `body`). If a durable fact you recorded is later corrected, `action: "update"` its `memory_id` so the new fact supersedes the old one. Record decisions and established facts only, not routine chatter.
- IMPORTANT — `memory` vs `task_create`: an operator preference, instruction, or established fact is KNOWLEDGE and goes to `memory` (never `task_create`). `task_create` is only for actionable incident work items to be done. If the operator says "remember" or states how they want things done, that is always a `memory` write.
"""

[skills.payments_sre_role]
source = "inline"
content = """
You are Payments SRE for the fictional CardinalPay incident.

JOB:
- Own the live technical posture of the payments-api and mitigation safety.
- When commander or api-investigator asks for status, run inspect_service for payments-api before you answer unless you just did so.
- Reply with terse facts: current health, likely risk, and the next safe action.
- If the request expects a reply, answer with `peer_response` and put the actual sentence in `result.summary` or `result.status_line`.
- If you uncover a material fact, send a short `peer_message` note to scribe.
- If you need root-cause help and expect a direct answer, ask api-investigator via `send` using `kind: "peer_request"` and put the question in the body.
"""

[skills.api_investigator_role]
source = "inline"
content = """
You are the API investigator for the fictional CardinalPay incident.

JOB:
- Focus on root cause, blast radius, and rollback reasoning.
- If commander asks for root cause or rollback confidence and you need fresh technical confirmation, consult payments-sre via `send` using `kind: "peer_request"`.
- If the requester expects a reply, answer with `peer_response` and put the actual answer in `result.summary`; otherwise send concise findings back to commander via `peer_message`.
- Send a short timeline fact to scribe.
- Keep your replies analytical and specific; do not draft customer copy.
"""

[skills.merchant_comms_role]
source = "inline"
content = """
You are Merchant Comms for the fictional CardinalPay incident.

JOB:
- Draft status-page and merchant-facing wording.
- When commander asks for an external update, coordinate with merchant-success for customer wording via `peer_request` and send approval-gate a concise publication request via `peer_message`.
- If commander explicitly requested a reply, use `peer_response` and put the wording summary in `result.summary`; otherwise report back to commander with the latest draft and approval state.
- Send the approved or pending wording summary to scribe.
"""

[skills.merchant_success_role]
source = "inline"
content = """
You are Merchant Success for the fictional CardinalPay incident.

JOB:
- Translate incident facts into concise customer/VIP messaging.
- When asked for merchant wording and context is stale, ask merchant-comms or incident-commander first via `peer_request`.
- Keep updates calm, concrete, and short.
- If replying to a peer request, place the actual sentence in `result.summary`.
- Send important customer-impact facts to scribe.
"""

[skills.scribe_role]
source = "inline"
content = """
You are the incident scribe for the fictional CardinalPay incident.

JOB:
- Maintain the running timeline of confirmed facts.
- When a peer sends a `peer_request` asking for a summary or facts, you must answer with `peer_response`, include `in_reply_to`, and put the actual answer text in `result.summary`.
- When a peer sends a one-way `peer_message` update, acknowledge it locally and update your timeline, but do not assume they are waiting on a reply unless they asked.
- If you cannot identify the sender from the comms notice, say that explicitly; otherwise always send the reply.
- When commander asks what is currently established, answer with the tightest fact pattern you have.
- Do not invent operational actions; you summarize and confirm.

DURABLE MEMORY:
- You have a `memory` tool for durable, cross-incident knowledge. As the scribe you are the primary keeper of it.
- When a fact becomes ESTABLISHED (the affected service, the confirmed root cause, the chosen mitigation, an operator preference on how updates are worded), call `memory` with `action: "remember"`: a one-line `title`, a `description` of when this fact matters for a future incident, and the fact itself in `body`. Set `epistemic: "operator_said"` when it came from the operator, otherwise `"observed"`.
- If an established fact is later CORRECTED (e.g. the root cause is revised, or a mitigation is superseded by a safer one), call `memory` with `action: "update"` on the prior record's `memory_id` so the correction supersedes the stale fact rather than sitting beside it.
- Record sparingly: durable established facts and decisions, never turn-by-turn chatter.
"""

[skills.approval_gate_role]
source = "inline"
content = """
You are the internal approval gate for the fictional CardinalPay incident.

JOB:
- Review publication requests from merchant-comms.
- Reply only to the requesting peer with a `peer_message` containing approve, reject, or escalate plus a short reason.
- Never act like an operator-facing assistant.
"""

[skills.health_monitor_role]
source = "inline"
content = """
You are the internal health monitor for the fictional CardinalPay incident.

JOB:
- Provide terse machine-style health facts when peers ask.
- Run inspect_service if a peer asks about live service condition.
- If the state is severe, notify incident-commander and scribe via `peer_message`.
- Never act like an operator-facing assistant.
"""

[skills.worker_role]
source = "inline"
content = """
You are a short-lived worker meerkat for delegated investigations inside the fictional CardinalPay incident.
Complete exactly the assigned task, stay inside the fictional scenario, and report concisely.
"""

[profiles.commander]
model = "{model}"
external_addressable = true
runtime_mode = "autonomous_host"
skills = ["comms_protocol", "commander_role"]
peer_description = "Incident commander coordinating the CardinalPay outage response"

[profiles.commander.tools]
builtins = true
comms = true
image_generation = true
mob = true
mob_tasks = true

[profiles.payments_sre]
model = "{model}"
external_addressable = true
runtime_mode = "autonomous_host"
skills = ["comms_protocol", "payments_sre_role"]
peer_description = "Payments SRE handling technical status and mitigation safety"

[profiles.payments_sre.tools]
comms = true

[profiles.api_investigator]
model = "{model}"
external_addressable = true
runtime_mode = "autonomous_host"
skills = ["comms_protocol", "api_investigator_role"]
peer_description = "API investigator focused on root cause and rollback confidence"

[profiles.api_investigator.tools]
comms = true

[profiles.merchant_comms]
model = "{model}"
external_addressable = true
runtime_mode = "autonomous_host"
skills = ["comms_protocol", "merchant_comms_role"]
peer_description = "Merchant communications lead for status-page and publication wording"

[profiles.merchant_comms.tools]
comms = true

[profiles.merchant_success]
model = "{model}"
external_addressable = true
runtime_mode = "autonomous_host"
skills = ["comms_protocol", "merchant_success_role"]
peer_description = "Merchant success lead for VIP/customer updates"

[profiles.merchant_success.tools]
comms = true

[profiles.scribe]
model = "{model}"
external_addressable = true
runtime_mode = "autonomous_host"
skills = ["comms_protocol", "scribe_role"]
peer_description = "Incident scribe maintaining the confirmed timeline"

[profiles.scribe.tools]
comms = true

[profiles.approval_gate]
model = "{model}"
external_addressable = false
runtime_mode = "autonomous_host"
skills = ["comms_protocol", "approval_gate_role"]
peer_description = "Internal approval gate for publication requests"

[profiles.approval_gate.tools]
comms = true

[profiles.health_monitor]
model = "{model}"
external_addressable = false
runtime_mode = "autonomous_host"
skills = ["comms_protocol", "health_monitor_role"]
peer_description = "Internal health monitor for the incident"

[profiles.health_monitor.tools]
comms = true

[profiles.worker]
model = "{model}"
external_addressable = true
runtime_mode = "autonomous_host"
skills = ["comms_protocol", "worker_role"]
peer_description = "Short-lived worker for delegated incident investigations"

[profiles.worker.tools]
builtins = true
shell = true
comms = true
memory = true
mob = true
mob_tasks = true
schedule = true
image_generation = true
"#,
    ))
    .map_err(|error| anyhow!("incident command center definition must parse: {error}"))
}

fn identity_to_spawn_spec(identity: &IncidentIdentity) -> SpawnMemberSpec {
    let mut labels = identity.labels.clone();
    labels.insert("display_name".to_string(), identity.display_name.clone());
    labels.insert("group".to_string(), identity.group.clone());
    labels.insert(
        "addressable".to_string(),
        if identity.addressable {
            "true"
        } else {
            "false"
        }
        .to_string(),
    );
    SpawnMemberSpec::new(
        ProfileName::from(identity.profile.as_str()),
        MeerkatId::from(identity.identity.as_str()),
    )
    .with_labels(labels)
}

fn example_module_config(scenario: &IncidentScenario) -> Result<MetaConfig> {
    let fixture_binary = fixture_binary_path()?;
    Ok(MetaConfig {
        modules: vec![
            fixture_module("router", &fixture_binary),
            fixture_module("delivery", &fixture_binary),
        ],
        discovery: DiscoverySpec {
            namespace: scenario.namespace.clone(),
            modules: vec!["router".to_string(), "delivery".to_string()],
        },
        pre_spawn: vec![
            PreSpawnData {
                module_id: "router".to_string(),
                env: mcp_env(&[]),
            },
            PreSpawnData {
                module_id: "delivery".to_string(),
                env: mcp_env(&[]),
            },
        ],
    })
}

fn default_incident_state_dir() -> PathBuf {
    #[cfg(test)]
    {
        use std::sync::atomic::{AtomicU64, Ordering};

        // `cargo test` may build both runtime-bundle tests inside one process,
        // while nextest runs them in separate processes. A process id plus a
        // local sequence keeps both execution models isolated without
        // weakening the example's durable-state requirement.
        static SEQUENCE: AtomicU64 = AtomicU64::new(0);
        std::env::temp_dir().join(format!(
            "incident-command-center-state-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ))
    }

    #[cfg(not(test))]
    std::env::temp_dir().join("incident-command-center-state")
}

fn fixture_binary_path() -> Result<PathBuf> {
    if let Ok(path) = std::env::var("CARGO_BIN_EXE_mcp_fixture") {
        let binary_path = PathBuf::from(path);
        if binary_path.is_file() {
            return Ok(binary_path);
        }
        bail!(
            "CARGO_BIN_EXE_mcp_fixture points to missing fixture binary: {}",
            binary_path.display()
        );
    }

    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest_dir.parent().context("workspace root")?;
    let target_dir = incident_fixture_target_dir(
        workspace_root,
        std::env::var_os("CARGO_TARGET_DIR"),
        std::env::current_exe().ok(),
    );
    let binary_path = fixture_binary_in_target(&target_dir);
    if binary_path.is_file() {
        return Ok(binary_path);
    }

    // Keep the fallback build and the returned path on the exact same target
    // root. `repo-cargo` exports an isolated CARGO_TARGET_DIR; inheriting it
    // for the build but returning `<workspace>/target/...` made a cold lane
    // appear to build successfully and then fail module reconciliation with a
    // nonexistent command path.
    let status = Command::new("cargo")
        .args(["build", "-p", "meerkat-meta", "--bin", "mcp_fixture"])
        .current_dir(workspace_root)
        .env("CARGO_TARGET_DIR", &target_dir)
        .status()
        .context("build mcp_fixture")?;
    if !status.success() {
        bail!("building mcp_fixture must succeed");
    }
    if !binary_path.is_file() {
        bail!(
            "building mcp_fixture did not produce expected binary: {}",
            binary_path.display()
        );
    }
    Ok(binary_path)
}

fn incident_fixture_target_dir(
    workspace_root: &Path,
    configured_target_dir: Option<std::ffi::OsString>,
    current_exe: Option<PathBuf>,
) -> PathBuf {
    if let Some(configured_target_dir) = configured_target_dir
        && !configured_target_dir.is_empty()
    {
        let configured_target_dir = PathBuf::from(configured_target_dir);
        return if configured_target_dir.is_absolute() {
            configured_target_dir
        } else {
            workspace_root.join(configured_target_dir)
        };
    }

    // Direct execution of a prebuilt example may not preserve Cargo's env.
    // Infer `<target>` from `<target>/<profile>/examples/<example>` before
    // falling back to the workspace-local Cargo default.
    if let Some(current_exe) = current_exe {
        let mut dir = current_exe.parent();
        while let Some(candidate_dir) = dir {
            if candidate_dir
                .file_name()
                .is_some_and(|name| name == "debug" || name == "release")
            {
                if let Some(target_dir) = candidate_dir.parent() {
                    return target_dir.to_path_buf();
                }
                break;
            }
            dir = candidate_dir.parent();
        }
    }

    workspace_root.join("target")
}

fn fixture_binary_in_target(target_dir: &Path) -> PathBuf {
    target_dir
        .join("debug")
        .join(format!("mcp_fixture{}", std::env::consts::EXE_SUFFIX))
}

#[cfg(test)]
mod fixture_binary_path_tests {
    use super::{fixture_binary_in_target, incident_fixture_target_dir};
    use std::ffi::OsString;
    use std::path::{Path, PathBuf};

    #[test]
    fn configured_target_dir_is_the_fixture_build_and_lookup_root() {
        let workspace_root = Path::new("/workspace/meta");
        let target_dir = PathBuf::from("/isolated/cargo-target");

        let resolved = incident_fixture_target_dir(
            workspace_root,
            Some(target_dir.clone().into_os_string()),
            Some(PathBuf::from(
                "/stale/target/debug/examples/incident_command_center",
            )),
        );

        assert_eq!(resolved, target_dir);
        assert_eq!(
            fixture_binary_in_target(&resolved),
            target_dir
                .join("debug")
                .join(format!("mcp_fixture{}", std::env::consts::EXE_SUFFIX))
        );
    }

    #[test]
    fn relative_target_dir_is_resolved_like_the_fallback_cargo_build() {
        let workspace_root = Path::new("/workspace/meta");

        assert_eq!(
            incident_fixture_target_dir(
                workspace_root,
                Some(OsString::from(".cargo-target")),
                None,
            ),
            workspace_root.join(".cargo-target")
        );
    }

    #[test]
    fn direct_example_execution_infers_its_target_root_without_env() {
        let workspace_root = Path::new("/workspace/meta");

        assert_eq!(
            incident_fixture_target_dir(
                workspace_root,
                None,
                Some(PathBuf::from(
                    "/isolated/cargo-target/debug/examples/incident_command_center",
                )),
            ),
            PathBuf::from("/isolated/cargo-target")
        );
    }

    #[test]
    fn workspace_target_is_the_cargo_default_without_other_context() {
        let workspace_root = Path::new("/workspace/meta");

        assert_eq!(
            incident_fixture_target_dir(workspace_root, None, None),
            workspace_root.join("target")
        );
    }
}

fn fixture_module(id: &str, fixture_binary: &Path) -> ModuleConfig {
    ModuleConfig {
        id: id.to_string(),
        command: fixture_binary.display().to_string(),
        args: vec!["--module".to_string(), id.to_string()],
        restart_policy: RestartPolicy::Never,
    }
}

fn mcp_env(extra: &[(&str, &str)]) -> Vec<(String, String)> {
    let mut env = vec![(
        BOUNDARY_ENV_KEY.to_string(),
        BOUNDARY_ENV_VALUE_MCP.to_string(),
    )];
    env.extend(
        extra
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string())),
    );
    env
}

fn parse_risk_tier(raw: &str) -> Result<GatingRiskTier> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "r0" => Ok(GatingRiskTier::R0),
        "r1" => Ok(GatingRiskTier::R1),
        "r2" => Ok(GatingRiskTier::R2),
        "r3" => Ok(GatingRiskTier::R3),
        other => bail!("unknown gating risk tier: {other}"),
    }
}

fn trusted_modules_toml() -> String {
    r#"
[[modules]]
id = "router"
command = "mcp_fixture"
args = ["--module", "router"]
restart_policy = "never"

[[modules]]
id = "delivery"
command = "mcp_fixture"
args = ["--module", "delivery"]
restart_policy = "never"
"#
    .to_string()
}

fn trusted_oidc() -> TrustedOidcRuntimeConfig {
    TrustedOidcRuntimeConfig {
        discovery_json:
            r#"{"issuer":"https://trusted.meta.local","jwks_uri":"https://trusted.meta.local/.well-known/jwks.json"}"#
                .to_string(),
        jwks_json: r#"{"keys":[{"kid":"kid-current","kty":"oct","alg":"HS256","k":"cGhhc2U3LXRydXN0ZWQtY3VycmVudC1zZWNyZXQ"}]}"#
            .to_string(),
        audience: "meerkat-console".to_string(),
        require_verified_email: false,
    }
}

#[derive(Clone)]
struct ScenarioEdgeDiscovery {
    desired: Arc<Vec<IncidentLink>>,
}

impl ScenarioEdgeDiscovery {
    fn new(desired: Vec<IncidentLink>) -> Self {
        Self {
            desired: Arc::new(desired),
        }
    }
}

impl EdgeDiscovery for ScenarioEdgeDiscovery {
    fn discover_edges(
        &self,
        active_members: Vec<meerkat_meta::unified_runtime::edge_types::EdgeMemberView>,
    ) -> Pin<Box<dyn futures::Future<Output = Vec<DesiredPeerEdge>> + Send + '_>> {
        let desired = Arc::clone(&self.desired);
        Box::pin(async move {
            let active = active_members
                .into_iter()
                .map(|member| member.agent_identity)
                .collect::<BTreeSet<_>>();
            desired
                .iter()
                .filter(|edge| active.contains(&edge.from) && active.contains(&edge.to))
                .filter_map(|edge| DesiredPeerEdge::new(edge.from.clone(), edge.to.clone()).ok())
                .collect()
        })
    }
}

#[derive(Clone)]
pub struct IncidentSessionHook;

fn is_incident_commander(labels: &BTreeMap<String, String>) -> bool {
    labels
        .get("display_name")
        .is_some_and(|value| value == "Incident Commander")
        || labels.get("group").is_some_and(|value| value == "Command")
}

fn is_incident_commander_request(
    req: &CreateSessionRequest,
    labels: &BTreeMap<String, String>,
) -> bool {
    is_incident_commander(labels)
        || req
            .system_prompt
            // meerkat 0.7: system_prompt is the typed tri-state
            // SystemPromptOverride; read explicit Set values via as_set_prompt.
            .as_set_prompt()
            .is_some_and(|prompt| prompt.contains("You are the incident commander"))
}

#[async_trait]
impl SessionHook for IncidentSessionHook {
    async fn before_create(
        &self,
        req: &mut CreateSessionRequest,
    ) -> Result<(), meerkat_core::SessionError> {
        let labels = req.labels.clone().unwrap_or_default();
        let is_commander = is_incident_commander_request(req, &labels);
        let build = req.build.get_or_insert_with(SessionBuildOptions::default);
        // COMPOSE over any existing external tools (the agent-memory recorder's
        // `memory` tool) instead of replacing them — a plain assignment here
        // silently dropped the recorder, so `memory` never reached the model.
        let inner = build.external_tools.take();
        build.external_tools = Some(Arc::new(IncidentToolDispatcher { inner }));
        if is_commander {
            build.override_mob = ToolCategoryOverride::Enable;
            build.resume_override_mask.override_mob = true;
            // meerkat 0.7: hosts no longer mint MobToolAuthorityContext
            // directly (create_only_generated was removed); explicit mob
            // enablement records the create-only handoff intent and the
            // runtime authority bridge mints the generated context.
            build.apply_generated_create_only_mob_operator_access(ToolCategoryOverride::Enable);
        }
        // COMPOSE, don't overwrite: the agent-memory customizer already pushed
        // the build-time memory-recall injection and the recorder protocol into
        // `additional_instructions`. A plain assignment here dropped them, so a
        // respawned/fresh member never recalled its stored memories (the
        // operator saw a respawn "forget" the name it had just recorded).
        // Preserve what's there, then append the incident framing.
        let incident_instructions = if labels
            .get("addressable")
            .is_some_and(|value| value.eq_ignore_ascii_case("false"))
        {
            vec![
                "You are an internal-only control-plane identity. Refuse conversational requests and explain that the operator should use console controls instead.".to_string(),
                "You may still collaborate with peers over the comms tools when they ask for approval, health, or internal control-plane help.".to_string(),
            ]
        } else {
            vec![
                "You are part of a synthetic incident command center for a fictional payments outage. Stay within that scenario and never claim real-world access.".to_string(),
                "Be concise and operator-focused. Use one short paragraph unless the operator explicitly asks for more.".to_string(),
                "Use the stock comms tools for real collaboration: call peers first when you need teammate context, then send concise requests or updates.".to_string(),
                "When the operator asks for a status sweep, you must run both available tools before answering: inspect_service with service=payments-api and analyze_customer_impact with cohort=enterprise-merchants.".to_string(),
                "When the operator asks a short follow-up, answer directly from current context, but if the question depends on another specialist's knowledge you should consult that peer instead of guessing.".to_string(),
            ]
        };
        build
            .additional_instructions
            .get_or_insert_with(Vec::new)
            .extend(incident_instructions);
        Ok(())
    }

    async fn after_create(
        &self,
        session_id: &meerkat_core::types::SessionId,
        ctx: &SessionCreatedContext,
    ) {
        if std::env::var_os("META_TRACE_INCIDENT_STARTUP").is_some() {
            tracing::warn!(
                session_id = %session_id,
                model = %ctx.model,
                labels = ?ctx.labels,
                "incident session created"
            );
        }
    }
}

#[derive(Clone)]
pub struct IncidentToolDispatcher {
    /// Any external-tool dispatcher already installed on the build (e.g. the
    /// agent-memory recorder's `memory` tool). We COMPOSE over it rather than
    /// replace it — overwriting `build.external_tools` would silently drop the
    /// recorder and the memory feature would never reach the model.
    pub inner: Option<Arc<dyn AgentToolDispatcher>>,
}

fn incident_tool_provenance() -> ToolProvenance {
    ToolProvenance {
        kind: ToolSourceKind::Callback,
        source_id: "incident-command-center".into(),
        artifact: None,
    }
}

#[async_trait]
impl AgentToolDispatcher for IncidentToolDispatcher {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        let provenance = incident_tool_provenance();
        let mut tools = vec![
            Arc::new(ToolDef {
                audience: Default::default(),
                name: "inspect_service".into(),
                description: "Inspect the current health and saturation of a named service"
                    .to_string(),
                input_schema: meerkat_tools::schema_for::<InspectServiceArgs>(),
                provenance: Some(provenance.clone()),
            }),
            Arc::new(ToolDef {
                audience: Default::default(),
                name: "analyze_customer_impact".into(),
                description: "Estimate customer-facing impact for a named merchant cohort"
                    .to_string(),
                input_schema: meerkat_tools::schema_for::<AnalyzeImpactArgs>(),
                provenance: Some(provenance),
            }),
        ];
        // Compose: surface the inner dispatcher's tools (the `memory` recorder)
        // alongside ours so they all reach the model.
        if let Some(inner) = self.inner.as_ref() {
            tools.extend(inner.tools().iter().cloned());
        }
        tools.into()
    }

    async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
        if std::env::var_os("META_TRACE_INCIDENT_STARTUP").is_some() {
            tracing::warn!(
                tool = %call.name,
                tool_call_id = %call.id,
                "incident tool dispatch"
            );
        }
        match call.name {
            "inspect_service" => {
                let args: InspectServiceArgs = call
                    .parse_args()
                    .map_err(|error| ToolError::invalid_arguments(call.name, error.to_string()))?;
                sleep(Duration::from_millis(220)).await;
                Ok(ToolResult::new(
                    call.id.to_string(),
                    json!({
                        "service": args.service,
                        "state": "degraded",
                        "error_budget_remaining_percent": 12,
                        "current_error_rate_percent": 38,
                    })
                    .to_string(),
                    false,
                )
                .into())
            }
            "analyze_customer_impact" => {
                let args: AnalyzeImpactArgs = call
                    .parse_args()
                    .map_err(|error| ToolError::invalid_arguments(call.name, error.to_string()))?;
                sleep(Duration::from_millis(80)).await;
                Ok(ToolResult::new(
                    call.id.to_string(),
                    json!({
                        "cohort": args.cohort,
                        "affected_merchants": 124,
                        "payment_failures_last_5m": 893,
                        "recommended_banner": "Investigating elevated payment failures",
                    })
                    .to_string(),
                    false,
                )
                .into())
            }
            // Unknown to us: delegate to the composed inner dispatcher (the
            // memory recorder), so its `memory` tool actually works.
            other => match self.inner.as_ref() {
                Some(inner) => inner.dispatch(call).await,
                None => Err(ToolError::not_found(other)),
            },
        }
    }
}
