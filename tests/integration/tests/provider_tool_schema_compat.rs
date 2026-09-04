//! Built-in tool schemas run through the provider request builders.
//!
//! Every first-party tool definition reachable from this crate is pushed
//! through the Gemini request builder and both OpenAI request builders
//! (Responses, and Chat Completions as used by the `openai_compatible`
//! transport). The deterministic matrix asserts the emitted `parameters` stay
//! inside what each provider's validator accepts: Gemini's `Schema` proto
//! rejects any undeclared field with `400 INVALID_ARGUMENT` (X1), and OpenAI
//! rejects root-level combinators with `invalid_function_parameters` (A2).
//!
//! The live tests (`#[ignore = "lane:e2e-live"]`) post the same tool families
//! to the real providers with a one-word prompt and require every request to
//! be accepted. They are registered in the e2e-live lane as the
//! `provider-tool-schema-compat` suite (`e2e_lanes::suite_spec`, driven by
//! `tests/e2e_live_lane.rs`), so `cargo e2e-live` / `make e2e-live` run them
//! with the OpenAI and Gemini keys as prerequisites; a missing key skips the
//! suite, or fails it under `MEERKAT_STRICT_E2E_PREREQS=1`, and the tests
//! apply the same rule when this binary is run alone:
//!
//! ```text
//! ./scripts/repo-cargo nextest run -p meerkat-integration-tests \
//!     --test provider_tool_schema_compat --run-ignored ignored-only
//! ```

#![allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeSet;
use std::path::Path;
use std::sync::Arc;

use futures::StreamExt;
use meerkat_client::openai_compatible::OpenAiCompatibleMode;
use meerkat_client::types::LlmClient;
use meerkat_client::{
    GeminiClient, LlmDoneOutcome, LlmEvent, LlmRequest, OpenAiClient, OpenAiCompatibleClient,
    OpenAiCompatibleClientOptions,
};
use meerkat_core::{AgentToolDispatcher, Message, SessionId, ToolDef, UserMessage};
use meerkat_integration_tests::e2e_lanes::strict_prereqs_enabled;
use serde_json::{Value, json};

/// Fields declared by Gemini's `Schema` proto message
/// (`google/ai/generativelanguage/v1beta/content.proto`), transcribed here
/// independently of the client so a drift in the client's allowlist is caught
/// by this matrix rather than mirrored by it. `title` is declared by the proto;
/// the client strips it anyway.
const GEMINI_SCHEMA_PROTO_FIELDS: &[&str] = &[
    "type",
    "format",
    "title",
    "description",
    "nullable",
    "enum",
    "maxItems",
    "minItems",
    "properties",
    "required",
    "minProperties",
    "maxProperties",
    "minimum",
    "maximum",
    "minLength",
    "maxLength",
    "pattern",
    "example",
    "anyOf",
    "propertyOrdering",
    "default",
    "items",
];

/// Keywords OpenAI's function-parameter validator rejects at the schema root
/// (`schema must have type 'object' and not have 'oneOf'/'anyOf'/'allOf'/
/// 'enum'/'not' at the top level`), transcribed independently of the client.
const OPENAI_ROOT_REJECTED_KEYWORDS: &[&str] = &["not", "oneOf", "anyOf", "allOf", "enum"];

const GEMINI_LIVE_MODEL: &str = "gemini-3.8-flash";
const OPENAI_LIVE_MODEL: &str = "gpt-5.5";

struct ToolFamily {
    name: &'static str,
    tools: Vec<Arc<ToolDef>>,
}

// ---------------------------------------------------------------------------
// Built-in tool collection
// ---------------------------------------------------------------------------

fn tool_def_from_listing(entry: &Value) -> Arc<ToolDef> {
    Arc::new(ToolDef {
        name: entry["name"]
            .as_str()
            .expect("tool listing entry carries a name")
            .into(),
        description: entry["description"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        input_schema: entry["inputSchema"].clone(),
        provenance: None,
    })
}

/// All 15 platform WorkGraph tools, including the attention-gated
/// `workgraph_attention_reassign` and `workgraph_policy_escalate` (object
/// `oneOf` argument shapes) and `workgraph_claim`.
fn workgraph_tools() -> Vec<Arc<ToolDef>> {
    meerkat::workgraph_tools_list()
        .iter()
        .map(tool_def_from_listing)
        .collect()
}

/// Schedule tools as the factory composes them (schedule is on by default in
/// `ToolsConfig`, so these ride along on plain `rkat run`).
fn schedule_tools() -> Vec<Arc<ToolDef>> {
    let dispatcher = meerkat::ScheduleToolDispatcher::new(meerkat::ScheduleService::new(Arc::new(
        meerkat::MemoryScheduleStore::default(),
    )));
    dispatcher.tools().to_vec()
}

/// Agent-facing mob tools (`delegate`, `council`, ...) exactly as a mob member
/// receives them from `AgentMobToolSurface`.
fn mob_agent_tools() -> Vec<Arc<ToolDef>> {
    // The surface advertises its catalog only behind generated mob tool
    // authority: a deserialized `MobToolAuthorityContext` is an untrusted
    // projection (serde cannot restore the generated seal) and yields an
    // empty catalog. Mint the authority through the runtime seam the factory
    // and the meerkat-mob-mcp tests use.
    let authority = meerkat_runtime::mob_operator_authority::create_only_mob_operator_authority()
        .expect("generated create-only mob authority");
    let surface = meerkat_mob_mcp::AgentMobToolSurface::new(
        meerkat_mob_mcp::MobMcpState::new_in_memory(),
        None,
        authority,
        GEMINI_LIVE_MODEL.to_string(),
        SessionId::new(),
        None,
        None,
        None,
    );
    surface.tools().to_vec()
}

/// The `mob_*` MCP-server inventory (`mob_wire`, `mob_spawn_member`, ...).
/// Its `oneOf` content/binding schemas are `json!` literals of their own; the
/// member operator tools in `meerkat-mob` define theirs separately, so both
/// are collected.
fn mob_mcp_tools() -> Vec<Arc<ToolDef>> {
    meerkat_mob_mcp::tools_list()
        .iter()
        .map(tool_def_from_listing)
        .collect()
}

/// Member-session operator tools from `meerkat-mob`, local flavor: the
/// `MobOperatorToolDispatcher` catalog a member receives when its profile
/// enables mob tools (`spawn_member` with the object-`oneOf` content
/// schema, `wire_members`, flow and status tools). This is what a Gemini mob
/// member is actually sent.
fn mob_operator_tools() -> Vec<Arc<ToolDef>> {
    meerkat_mob::member_operator_tool_defs_for_test()
}

/// Member-session operator tools from `meerkat-mob`, remote flavor: what the
/// member upcall surface advertises to a member placed on another host.
fn mob_remote_operator_tools() -> Vec<Arc<ToolDef>> {
    meerkat_mob::remote_member_operator_tool_defs_for_test()
}

/// The `memory_search` tool exactly as the factory mounts it for a session
/// with a memory store: a schemars-derived input whose `Option<usize>` limit
/// emits `type: ["integer", "null"]` and `format: "uint"`, the shapes the
/// lowering exists for. The facade re-export is gated on its
/// `memory-store-session` feature, which meerkat-rpc (a dependency of this
/// crate) enables, so feature unification makes it visible here.
fn memory_tools() -> Vec<Arc<ToolDef>> {
    let dispatcher = meerkat::MemorySearchDispatcher::new(
        Arc::new(meerkat::SimpleMemoryStore::new()),
        meerkat_core::memory::MemorySearchScope::for_session(SessionId::new()),
    );
    dispatcher.tools().to_vec()
}

fn comms_tools() -> Vec<Arc<ToolDef>> {
    meerkat_comms::comms_tool_defs()
}

/// Builtin task, utility and shell tools from the composite dispatcher.
fn builtin_tools(project_root: &Path) -> Vec<Arc<ToolDef>> {
    let shell_config = meerkat_tools::builtin::shell::ShellConfig {
        enabled: true,
        project_root: project_root.to_path_buf(),
        ..Default::default()
    };
    // The shell tools are `default_enabled() == false`; a shell-enabled build
    // switches them on through the policy layer exactly like this
    // (`AgentFactory` builtin_config construction).
    let builtin_config = meerkat::BuiltinToolConfig {
        policy: meerkat::ToolPolicyLayer::new()
            .enable_tool("shell")
            .enable_tool("shell_job_status")
            .enable_tool("shell_jobs")
            .enable_tool("shell_job_cancel"),
        ..Default::default()
    };
    let dispatcher = meerkat::CompositeDispatcher::new(
        Arc::new(meerkat::MemoryTaskStore::new()),
        &builtin_config,
        Some(project_root.to_path_buf()),
        Some(shell_config),
        None,
        None,
    )
    .expect("composite dispatcher with builtin tools");
    dispatcher.tools().to_vec()
}

fn builtin_tool_families(project_root: &Path) -> Vec<ToolFamily> {
    vec![
        ToolFamily {
            name: "workgraph",
            tools: workgraph_tools(),
        },
        ToolFamily {
            name: "schedule",
            tools: schedule_tools(),
        },
        ToolFamily {
            name: "mob-agent",
            tools: mob_agent_tools(),
        },
        ToolFamily {
            name: "mob-mcp",
            tools: mob_mcp_tools(),
        },
        ToolFamily {
            name: "mob-operator",
            tools: mob_operator_tools(),
        },
        ToolFamily {
            name: "mob-operator-remote",
            tools: mob_remote_operator_tools(),
        },
        ToolFamily {
            name: "comms",
            tools: comms_tools(),
        },
        ToolFamily {
            name: "memory",
            tools: memory_tools(),
        },
        ToolFamily {
            name: "builtin",
            tools: builtin_tools(project_root),
        },
    ]
}

// ---------------------------------------------------------------------------
// Request builders
// ---------------------------------------------------------------------------

fn request_with_tools(model: &str, tools: Vec<Arc<ToolDef>>) -> LlmRequest {
    LlmRequest::new(
        model,
        vec![Message::User(UserMessage::text("hello".to_string()))],
    )
    .with_max_tokens(256)
    .with_tools(tools)
}

fn compat_options() -> OpenAiCompatibleClientOptions {
    OpenAiCompatibleClientOptions {
        supports_temperature: false,
        supports_thinking: false,
        supports_reasoning: false,
        supports_image_tool_results: true,
    }
}

fn gemini_parameters(tool: &Arc<ToolDef>) -> Result<Value, String> {
    let body = GeminiClient::new("test-key".to_string())
        .build_request_body(&request_with_tools(
            GEMINI_LIVE_MODEL,
            vec![Arc::clone(tool)],
        ))
        .map_err(|error| format!("{error:?}"))?;
    Ok(body["tools"][0]["functionDeclarations"][0]["parameters"].clone())
}

fn openai_responses_parameters(tool: &Arc<ToolDef>) -> Result<Value, String> {
    let body = OpenAiClient::new("test-key".to_string())
        .build_request_body(&request_with_tools(
            OPENAI_LIVE_MODEL,
            vec![Arc::clone(tool)],
        ))
        .map_err(|error| format!("{error:?}"))?;
    Ok(body["tools"][0]["parameters"].clone())
}

fn openai_chat_parameters(tool: &Arc<ToolDef>) -> Result<Value, String> {
    let client = OpenAiCompatibleClient::new_with_options(
        OpenAiCompatibleMode::ChatCompletions,
        OPENAI_LIVE_MODEL.to_string(),
        "https://example.test/v1".to_string(),
        None,
        compat_options(),
    );
    let body = client
        .build_chat_completions_body(&request_with_tools(
            OPENAI_LIVE_MODEL,
            vec![Arc::clone(tool)],
        ))
        .map_err(|error| format!("{error:?}"))?;
    Ok(body["tools"][0]["function"]["parameters"].clone())
}

/// Walk the schema-bearing positions of a lowered Gemini `parameters` value
/// and record every field the Schema proto does not declare, every position
/// that holds something other than a Schema message (a JSON Schema boolean
/// such as `true`, or a tuple-form `items` list where `Schema.items` is a
/// single message), and every `enum` member that is not a string
/// (`Schema.enum` is `repeated string`).
fn collect_non_gemini_fields(value: &Value, path: &str, offending: &mut Vec<String>) {
    let Value::Object(obj) = value else {
        offending.push(format!("{path}: {value} is not a Schema object"));
        return;
    };
    for (key, child) in obj {
        if !GEMINI_SCHEMA_PROTO_FIELDS.contains(&key.as_str()) {
            offending.push(format!("{path}/{key}"));
        }
        match key.as_str() {
            "properties" => {
                if let Value::Object(properties) = child {
                    for (name, schema) in properties {
                        collect_non_gemini_fields(
                            schema,
                            &format!("{path}/properties/{name}"),
                            offending,
                        );
                    }
                }
            }
            "items" => match child {
                Value::Object(_) => {
                    collect_non_gemini_fields(child, &format!("{path}/items"), offending);
                }
                other => offending.push(format!(
                    "{path}/items: {other} is not a single Schema message"
                )),
            },
            "anyOf" => match child {
                Value::Array(entries) => {
                    for (index, entry) in entries.iter().enumerate() {
                        collect_non_gemini_fields(
                            entry,
                            &format!("{path}/anyOf/{index}"),
                            offending,
                        );
                    }
                }
                other => offending.push(format!(
                    "{path}/anyOf: {other} is not a list of Schema messages"
                )),
            },
            "enum" => match child {
                Value::Array(members) => {
                    for (index, member) in members.iter().enumerate() {
                        if !member.is_string() {
                            offending.push(format!(
                                "{path}/enum/{index}: {member} is not a string (Schema.enum is repeated string)"
                            ));
                        }
                    }
                }
                other => offending.push(format!("{path}/enum: {other} is not a list")),
            },
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Deterministic matrix
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "current_thread")]
async fn builtin_tool_schemas_lower_into_provider_accepted_shapes() {
    let temp = tempfile::tempdir().expect("tempdir");
    let families = builtin_tool_families(temp.path());

    // Coverage floor: the tools whose schemas motivated the lowering must be
    // in the matrix, in the family that actually owns them (the `mob_*` names
    // exist in three separately defined catalogs), otherwise a green run
    // proves nothing about them.
    let covered: BTreeSet<(String, String)> = families
        .iter()
        .flat_map(|family| {
            family
                .tools
                .iter()
                .map(|tool| (family.name.to_string(), tool.name.to_string()))
        })
        .collect();
    for (family, expected) in [
        ("workgraph", "workgraph_claim"),
        ("workgraph", "workgraph_attention_reassign"),
        ("workgraph", "workgraph_policy_escalate"),
        ("schedule", "meerkat_schedule_create"),
        ("schedule", "meerkat_schedule_update"),
        ("mob-agent", "delegate"),
        ("mob-agent", "council"),
        ("mob-mcp", "mob_wire"),
        ("mob-mcp", "mob_spawn_member"),
        ("mob-operator", "spawn_member"),
        ("mob-operator", "wire_members"),
        ("mob-operator-remote", "spawn_member"),
        ("comms", "send_message"),
        ("comms", "peers"),
        ("memory", "memory_search"),
        ("builtin", "shell"),
        ("builtin", "task_create"),
        ("builtin", "apply_patch"),
    ] {
        assert!(
            covered.contains(&(family.to_string(), expected.to_string())),
            "matrix must cover `{expected}` in family `{family}`; covered: {covered:?}"
        );
    }

    let mut failures = Vec::new();
    for family in &families {
        assert!(
            !family.tools.is_empty(),
            "{} family produced no tools",
            family.name
        );
        for tool in &family.tools {
            let label = format!("{}/{}", family.name, tool.name);

            match gemini_parameters(tool) {
                Ok(parameters) => {
                    if parameters["type"] != json!("object") {
                        failures.push(format!(
                            "{label}: gemini root type is {} not object",
                            parameters["type"]
                        ));
                    }
                    let mut offending = Vec::new();
                    collect_non_gemini_fields(&parameters, "", &mut offending);
                    if !offending.is_empty() {
                        failures.push(format!(
                            "{label}: gemini parameters carry undeclared Schema fields {offending:?}"
                        ));
                    }
                }
                Err(error) => {
                    failures.push(format!("{label}: gemini request builder failed: {error}"));
                }
            }

            for (path_name, result) in [
                ("openai-responses", openai_responses_parameters(tool)),
                ("openai-chat", openai_chat_parameters(tool)),
            ] {
                match result {
                    Ok(parameters) => {
                        let Some(root) = parameters.as_object() else {
                            failures.push(format!(
                                "{label}: {path_name} parameters are not an object: {parameters}"
                            ));
                            continue;
                        };
                        if root.get("type") != Some(&json!("object")) {
                            failures.push(format!(
                                "{label}: {path_name} root type is {:?} not object",
                                root.get("type")
                            ));
                        }
                        for keyword in OPENAI_ROOT_REJECTED_KEYWORDS {
                            if root.contains_key(*keyword) {
                                failures.push(format!(
                                    "{label}: {path_name} parameters carry root-level `{keyword}`"
                                ));
                            }
                        }
                    }
                    Err(error) => {
                        failures.push(format!(
                            "{label}: {path_name} request builder failed: {error}"
                        ));
                    }
                }
            }
        }
    }

    assert!(
        failures.is_empty(),
        "provider tool schema matrix failures:\n{}",
        failures.join("\n")
    );
}

// ---------------------------------------------------------------------------
// Live acceptance (lane: e2e-live, ignored by default)
// ---------------------------------------------------------------------------

fn first_env(vars: &[&str]) -> Option<String> {
    vars.iter().find_map(|name| std::env::var(name).ok())
}

fn gemini_api_key() -> Option<String> {
    first_env(&["RKAT_GEMINI_API_KEY", "GEMINI_API_KEY", "GOOGLE_API_KEY"])
}

fn openai_api_key() -> Option<String> {
    first_env(&["RKAT_OPENAI_API_KEY", "OPENAI_API_KEY"])
}

/// A missing provider key skips the live test, unless the lane runs strict
/// (`MEERKAT_STRICT_E2E_PREREQS=1`), in which case a skip is a failure: the
/// same rule `e2e_lanes` applies to catalog scenarios and suites.
fn skip_or_fail_on_missing_key(test: &str, key: &str) {
    let message = format!("SKIP {test} reason=missing_api_key ({key})");
    assert!(
        !strict_prereqs_enabled(),
        "{message}: MEERKAT_STRICT_E2E_PREREQS turns a missing prerequisite into a failure"
    );
    eprintln!("{message}");
}

/// Post one tool family with a one-word prompt and report whether the provider
/// accepted the request. Any terminal outcome other than a provider error
/// (text, a tool call, or a max-token stop) counts as accepted; a rejected
/// schema surfaces as `Done(Error)` carrying the provider's 400 body verbatim.
async fn post_tool_family(
    client: &dyn LlmClient,
    model: &str,
    family: &ToolFamily,
) -> Result<(), String> {
    let request = request_with_tools(model, family.tools.clone());
    let mut stream = client.stream(&request);
    while let Some(event) = stream.next().await {
        match event {
            Ok(LlmEvent::Done {
                outcome: LlmDoneOutcome::Success { .. },
            }) => return Ok(()),
            Ok(LlmEvent::Done {
                outcome: LlmDoneOutcome::Error { error },
            }) => return Err(format!("provider Done(Error): {error:?}")),
            Ok(_) => {}
            Err(error) => return Err(format!("stream error: {error:?}")),
        }
    }
    Err("stream ended without Done".to_string())
}

async fn assert_live_acceptance(
    provider: &str,
    model: &str,
    client: &dyn LlmClient,
    families: &[ToolFamily],
) {
    let mut failures = Vec::new();
    for family in families {
        // Gemini names a rejected declaration by index
        // (`tools[0].function_declarations[N]`); print the indexed names so the
        // log resolves N to a tool without a second run.
        let indexed: Vec<String> = family
            .tools
            .iter()
            .enumerate()
            .map(|(index, tool)| format!("[{index}] {}", tool.name))
            .collect();
        eprintln!(
            "-- {provider} model={model} family={} tools={}",
            family.name,
            indexed.join(", ")
        );
        match post_tool_family(client, model, family).await {
            Ok(()) => eprintln!("   accepted ({} tools)", family.tools.len()),
            Err(error) => {
                eprintln!("   REJECTED: {error}");
                failures.push(format!("{provider}/{}: {error}", family.name));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "{provider} rejected built-in tool schemas:\n{}",
        failures.join("\n")
    );
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "lane:e2e-live"]
async fn live_gemini_accepts_every_builtin_tool_family() {
    let Some(api_key) = gemini_api_key() else {
        skip_or_fail_on_missing_key(
            "live_gemini_accepts_every_builtin_tool_family",
            "GEMINI_API_KEY",
        );
        return;
    };
    let temp = tempfile::tempdir().expect("tempdir");
    let families = builtin_tool_families(temp.path());
    let client = GeminiClient::new(api_key);
    assert_live_acceptance("gemini", GEMINI_LIVE_MODEL, &client, &families).await;
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "lane:e2e-live"]
async fn live_openai_responses_accepts_every_builtin_tool_family() {
    let Some(api_key) = openai_api_key() else {
        skip_or_fail_on_missing_key(
            "live_openai_responses_accepts_every_builtin_tool_family",
            "OPENAI_API_KEY",
        );
        return;
    };
    let temp = tempfile::tempdir().expect("tempdir");
    let families = builtin_tool_families(temp.path());
    let client = OpenAiClient::new(api_key);
    assert_live_acceptance("openai-responses", OPENAI_LIVE_MODEL, &client, &families).await;
}

/// The Chat Completions path is what the self-hosted `openai_compatible`
/// transport speaks; pointed at api.openai.com it exercises the validator that
/// rejected the root-level `not`.
#[tokio::test(flavor = "current_thread")]
#[ignore = "lane:e2e-live"]
async fn live_openai_chat_completions_accepts_every_builtin_tool_family() {
    let Some(api_key) = openai_api_key() else {
        skip_or_fail_on_missing_key(
            "live_openai_chat_completions_accepts_every_builtin_tool_family",
            "OPENAI_API_KEY",
        );
        return;
    };
    let temp = tempfile::tempdir().expect("tempdir");
    let families = builtin_tool_families(temp.path());
    let client = OpenAiCompatibleClient::new_with_options(
        OpenAiCompatibleMode::ChatCompletions,
        OPENAI_LIVE_MODEL.to_string(),
        "https://api.openai.com/v1".to_string(),
        Some(api_key),
        compat_options(),
    );
    assert_live_acceptance("openai-chat", OPENAI_LIVE_MODEL, &client, &families).await;
}
