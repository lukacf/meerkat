//! Actual CLI in an owned PTY, with ordinary native self-hosted configuration.
//! Model and the paired host fixtures use loopback endpoints. The explicitly ignored
//! public-service case also reads live public Hacker News through the supplied
//! integration. No OS store, user terminal or provider account. PTY cases require the
//! assembled, pin-built synthetic resource prefix. Explicit supplied-library cases
//! use their selected real prefix; stdio has its separate fixture contract.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    os::{fd::OwnedFd, unix::process::CommandExt},
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus, Stdio},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use nix::{
    errno::Errno,
    fcntl::{FcntlArg, OFlag, fcntl},
    pty::openpty,
    sys::{
        signal::{Signal, killpg},
        termios::{LocalFlags, Termios, tcgetattr},
    },
    unistd::{Pid, read, write},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::oneshot,
    time::{sleep, timeout},
};

const BOUND: Duration = Duration::from_secs(20);
const SUPPLIED_LIBRARY_BOUND: Duration = Duration::from_secs(360);
const REALM: &str = "cli-frontend";
const CODE: &str = "842619";
const DEVICE: &str = "cli-fixture-device-cookie";
const PAIR: &str = "cli-fixture-pair-cookie";
const TOKEN: &str = "cli-fixture-self-hosted-bearer";
const OAUTH_CODE: &str = "fixture-private-provider-code";
const OAUTH_TOKEN: &str = "fixture-private-provider-access-token";
const OAUTH_REFRESH: &str = "fixture-private-provider-refresh-token";
const COPILOT_DEVICE_CODE: &str = "fixture-private-copilot-device-code";
const COPILOT_USER_CODE: &str = "TEST-ABCD";
const OAUTH_MODEL: &str = "claude-opus-5-5";
const OAUTH_PROMPT: &str = "Confirm this original setup request reached the selected model.";
const OAUTH_ANSWER: &str = "The original setup request reached the selected native model target.";
const SETUP_HELPER_REVIEW: &str =
    "Allow the supplied setup helper to prepare choices in this realm?";
const SETUP_MODEL_REVIEW: &str = "Allow this setup assistant to use the selected model and tools?";
const DEFAULT_REQUEST: &str =
    "Help me set up my Telegram frontend, starting with supplied setup guidance.";
const SERVICE_REQUEST: &str =
    "Read supplied service setup guidance first, then help me set up my Telegram frontend.";
const LIBRARY_REQUEST: &str = "Read the supplied integration catalog and one exact entry, then help me set up my Telegram frontend. Do not install any service integration.";
const PREBOUND_LIBRARY_REQUEST: &str = "Check my selected paired host's frontend inventory, then read the supplied integration catalog and one exact entry before preparing my Telegram frontend. Do not install any service integration.";
const FOLLOWUP: &str = "Keep the same host and confirm the frontend inventory once more.";
const FOLLOWUP_ANSWER: &str =
    "The same captured host still reports its frontend inventory. No second pairing was started.";
const ANSWER: &str = "The review request was submitted. Paired host activation is separate.";
const INTENTS: &str = "/console/extensions/secrets/intents";
const INTENT_ID: &str = "esi_aaaaaaaaaaaaaaaaaaaa";
const ENTRY_PATH: &str = "/console/extensions/secrets/intents/esi_aaaaaaaaaaaaaaaaaaaa/entry";
const SECRET_ANSWER: &str = "The paired host reports generation 1 committed but not live. Provider authentication and message delivery remain unverified.";
const REVIEW_REQUEST: &str = "Show that prepared frontend draft's private review link again. Do not prepare or submit it again.";
const REVIEW_ANSWER: &str = "The paired host still retains the draft. Its review link was queued for the terminal; no preparation or submission was repeated.";
const RECOVERY_REQUEST: &str = "Prepare and submit my frontend review request once.";
const STATUS_REQUEST: &str = "Check only that exact request's status. Do not submit it again.";
const STATUS_ANSWER: &str = "The paired host reports the review request submitted. The lost POST remains uncertain; it was not sent again.";
const RECIPE_REQUEST: &str = "Show the first supplied integration recipe and its exact service capabilities, then look at the next entry. Investigate only.";
const RECIPE_ANSWER: &str = "The supplied recipe metadata describes scoped service capabilities. Installation and account readiness remain unverified.";
const RECIPE_FAILED_REQUEST: &str =
    "Explain the source requirements for that exact recipe. Investigate only.";
const RECIPE_PROVIDER_FAILURE: &str = "fixture rejected this model request";
const DOCUMENT_REQUEST: &str = "Read https://127.0.0.1:9/guide?lang=en#install as source evidence, if it is a public document.";
const DOCUMENT_URL: &str = "https://127.0.0.1:9/guide?lang=en#install";
const DOCUMENT_ANSWER: &str =
    "The reader refused the private destination. No source document was fetched.";
const RECIPE_FOLLOWUP: &str =
    "Keep this same conversation and remind me what was actually verified.";
const RECIPE_FOLLOWUP_ANSWER: &str =
    "Only the exact public recipe metadata was read in this conversation.";
const EXISTING_REALM: &str = "kept-family";
const EXISTING_REALM_HISTORY: &str = "Keep this original realm's private conversation unchanged.";
const ROLE_REPO: &str = "https://github.com/fixture/example-device.git";
const ROLE_REQUEST: &str = "Inspect the device extension at https://github.com/fixture/example-device.git tag v0.1.0 in package extension on my paired host. Ask me which supported roles to use before preparing anything.";
const ROLE_QUESTION: &str = "The paired host offers service connector only, agent frontend only, or both. Which roles do you want? The frontend choices also need your identity-to-device map.";
const ROLE_CHOICE: &str = "Use both advertised roles and map identity:member-2 to device:fixture-room. Prepare and submit that exact proposal for review.";
const ROLE_ANSWER: &str = "The paired host reports attempt 2 built for both requested roles. Build and review evidence does not establish activation, daemon operation or live voice readiness.";

#[derive(Clone, Copy, PartialEq, Eq)]
enum DeferredMode {
    FrontendOnly,
    SyntheticServices,
    SuppliedLibrary,
    PreboundSuppliedLibrary,
}

impl DeferredMode {
    fn services(self) -> bool {
        self != Self::FrontendOnly
    }

    fn supplied(self) -> bool {
        matches!(self, Self::SuppliedLibrary | Self::PreboundSuppliedLibrary)
    }

    fn prebound(self) -> bool {
        self == Self::PreboundSuppliedLibrary
    }

    fn prompt(self) -> &'static str {
        match self {
            Self::FrontendOnly => DEFAULT_REQUEST,
            Self::SyntheticServices => SERVICE_REQUEST,
            Self::SuppliedLibrary => LIBRARY_REQUEST,
            Self::PreboundSuppliedLibrary => PREBOUND_LIBRARY_REQUEST,
        }
    }

    fn service_tools(self) -> &'static [&'static str] {
        match self {
            Self::FrontendOnly => &[],
            Self::SyntheticServices => &["setup_help", "setup_install"],
            Self::SuppliedLibrary | Self::PreboundSuppliedLibrary => &[
                "setup_catalog",
                "setup_catalog_entry",
                "setup_state",
                "setup_install",
            ],
        }
    }

    fn frontend_start(self) -> usize {
        match self {
            Self::FrontendOnly => 2,
            Self::SyntheticServices => 5,
            Self::SuppliedLibrary | Self::PreboundSuppliedLibrary => 7,
        }
    }
}

struct Request {
    method: String,
    path: String,
    headers: BTreeMap<String, String>,
    body: Value,
}

async fn request(stream: &mut TcpStream) -> Request {
    let mut bytes = Vec::new();
    let boundary = loop {
        if let Some(index) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
            break index + 4;
        }
        let mut buffer = [0; 4096];
        let count = stream.read(&mut buffer).await.unwrap();
        assert!(count > 0 && bytes.len() + count < 1024 * 1024);
        bytes.extend_from_slice(&buffer[..count]);
    };
    let head = std::str::from_utf8(&bytes[..boundary]).unwrap();
    let mut lines = head.split("\r\n");
    let mut first = lines.next().unwrap().split_whitespace();
    let method = first.next().unwrap().to_owned();
    let path = first.next().unwrap().to_owned();
    let headers: BTreeMap<_, _> = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(key, value)| (key.to_ascii_lowercase(), value.trim().to_owned()))
        .collect();
    let length: usize = headers
        .get("content-length")
        .map(|v| v.parse().unwrap())
        .unwrap_or(0);
    assert!(length < 1024 * 1024 && !headers.contains_key("transfer-encoding"));
    while bytes.len() < boundary + length {
        let mut buffer = [0; 4096];
        let count = stream.read(&mut buffer).await.unwrap();
        assert!(count > 0);
        bytes.extend_from_slice(&buffer[..count]);
    }
    Request {
        method,
        path,
        headers,
        body: if length == 0 {
            Value::Null
        } else {
            serde_json::from_slice(&bytes[boundary..boundary + length]).unwrap()
        },
    }
}

fn candidate() -> Value {
    json!({"extension_id":"telegram","channel_id":"telegram:primary","ingress":"polling",
        "participants":{"approved_groups":["-1001234567890"]}})
}

fn inventory() -> Value {
    json!({"current":{"activation_id":"210","policy_digest":"sha256:cli-fixture"},
        "extensions":[{"extension_id":"telegram","enabled":true,"wheel_present":true,
            "channels":[{"channel_id":"telegram:primary","ingress":["polling","webhook"],
                "secrets":[{"name":"TELEGRAM_BOT_TOKEN","present":false}],
                "approved_groups":{"declared":["-1001234567890"],"discovery":"unavailable"},
                "senders":{"identities":["identity:parent-1"],"informational":true}}]}],
        "routing":"sender_identity"})
}

fn prepared() -> Value {
    let before = json!({"id":"telegram:primary","channel_type":"telegram","ingress":"polling",
        "sender_id":"sender","provider_account":"household","principal_subject_id":"paired_host",
        "source_event_type":"string","secret_names":[],"dedup_horizon_seconds":3600,
        "features":[],"approved_groups":[],"polling":{"interval_seconds":1},
        "inbound_limits":{"max_text_utf8_bytes":100,"max_caption_utf8_bytes":100,"max_parts":1,
            "max_part_bytes":100,"max_event_bytes":100,"max_channel_stored_bytes":100,"allowed_mime_types":["text/plain"]},
        "outbound_limits":{"max_text_utf8_bytes":100,"max_caption_utf8_bytes":100,
            "max_media_bytes":100,"allowed_media_types":["text/plain"]}});
    let mut after = before.clone();
    after["approved_groups"] = candidate()["participants"]["approved_groups"].clone();
    let mut change = json!({"base":{"activation_id":"210","policy_digest":"sha256:cli-fixture",
        "ledger_digest":format!("sha256:{}", "1".repeat(64))},
        "ledger_patch":{"path":"deployment/extensions.toml","extension_id":"telegram",
            "channel_id":"telegram:primary","before":before,"after":after},
        "unified_diff":"--- a/deployment/extensions.toml\n+++ b/deployment/extensions.toml\n@@ -1 +1 @@\n-approved_groups = []\n+approved_groups = [\"-1001234567890\"]\n"});
    change["change_digest"] = json!(format!(
        "paired-host-change-v1:sha256:{:x}",
        Sha256::digest(serde_json::to_vec(&change).unwrap())
    ));
    json!({"valid":true,"problems":[],
        "missing_prerequisites":[{"kind":"secret","name":"TELEGRAM_BOT_TOKEN"}],
        "current":{"activation_id":"210","policy_digest":"sha256:cli-fixture"},
        "candidate":{"channel_row_digest":"sha256:row","policy_digest":"fixture-policy-v1:sha256:policy",
            "compile_depth":"policy_bundle"},"compiled_is_not_ready":true,"change":change})
}

fn tool_result(body: &Value, id: &str) -> Value {
    let message = body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|message| message["role"] == "tool" && message["tool_call_id"] == id)
        .expect("the native HTTP request must contain the preceding tool result");
    serde_json::from_str(message["content"].as_str().unwrap()).unwrap()
}

// Synthetic PR457 host observations only. No source fetch, build or device use.
fn role_inspection() -> Value {
    json!({"repo":ROLE_REPO,"tag":"v0.1.0","commit":"b".repeat(40),
        "package_path":"extension","distribution":"example-ext-device","version":"0.1.0",
        "entry_point":{"group":"example_host.extensions","name":"device",
            "object":"example_ext_device.extension:DeviceExtension"},
        "declared":{},"already_declared":false,
        "unverified":["manifest","facades","role_contract","host_bindings","secrets"],
        "roles":{"selections":[
            {"selected_roles":["service_connector"],"daemon":{"voice_mode":"disabled"},
                "required_facades":["daemon_control.v1"],"voice_map_entries":0},
            {"selected_roles":["agent_frontend"],"daemon":{"voice_mode":"gateway"},
                "required_facades":["daemon_control.v1","live.v1","media.v1","notify.v1"],"voice_map_entries":1},
            {"selected_roles":["agent_frontend","service_connector"],"daemon":{"voice_mode":"gateway"},
                "required_facades":["daemon_control.v1","live.v1","media.v1","notify.v1"],"voice_map_entries":1}
        ]},
        "prepare":{"new_extension":{"distribution":"example-ext-device","version":"0.1.0",
            "source":{"repo":ROLE_REPO,"tag":"v0.1.0","commit":"b".repeat(40),"package_path":"extension"},
            "roles":{"selected_roles":null,"daemon":{"voice_mode":null},"required_facades":null,"voice_map":null}}}})
}

fn combined_role_proposal(observed: &Value) -> Value {
    let selected = observed["roles"]["selections"]
        .as_array()
        .unwrap()
        .iter()
        .find(|choice| choice["selected_roles"] == json!(["agent_frontend", "service_connector"]))
        .expect("the chosen combination must come from the host's actual inspection");
    assert_eq!(selected["voice_map_entries"], 1);
    let mut proposal = observed["prepare"]["new_extension"].clone();
    proposal["roles"] = json!({"selected_roles":selected["selected_roles"],
        "daemon":selected["daemon"],"required_facades":selected["required_facades"],
        "voice_map":{"identity:member-2":"device:fixture-room"}});
    proposal
}

fn role_prepared() -> Value {
    let digest = format!("paired-host-change-v1:sha256:{}", "a".repeat(64));
    json!({"valid":true,"problems":[],
        "review_url":format!("/console/extensions/frontend/review/{digest}"),
        "change":{"change_digest":digest,"kind":"new_extension",
            "base":{"activation_id":"210","policy_digest":"sha256:cli-fixture","ledger_digest":"fixture-ledger"},
            "proposal":combined_role_proposal(&role_inspection()),
            "unverified":role_inspection()["unverified"],
            "prerequisites":[{"kind":"build_and_review","state":"pending"}],
            "unified_summary":"Request host-owned build and review of both selected roles. Activation and readiness remain unverified."}})
}

fn role_built() -> Value {
    let proposal = combined_role_proposal(&role_inspection());
    json!({"request_id":role_prepared()["change"]["change_digest"],"state":"built","kind":"new_extension",
        "submitted_at":"fixture-time","staged_at":"fixture-stage","attempt":2,
        "built":{"extension_id":"device","distribution":proposal["distribution"],"version":proposal["version"],
            "source_commit":proposal["source"]["commit"],"package_path":proposal["source"]["package_path"],
            "wheel_sha256":"c".repeat(64),"release_evidence_sha256":"d".repeat(64),
            "channel_template":null,"facades":proposal["roles"]["required_facades"],"secret_names":[]},
        "prerequisites":[]})
}

fn role_model_response(request: &Request, phase: usize) -> String {
    assert_eq!(request.method, "POST");
    assert_eq!(request.path, "/v1/chat/completions");
    assert_eq!(request.headers["authorization"], format!("Bearer {TOKEN}"));
    assert!(!request.headers.contains_key("cookie"));
    assert_eq!(request.body["model"], "cli-fixture-remote");
    assert_eq!(request.body["stream"], true);
    let serialized = request.body.to_string();
    for protected in [
        CODE,
        DEVICE,
        PAIR,
        TOKEN,
        "/console/extensions/frontend/review/",
    ] {
        assert!(
            !serialized.contains(protected),
            "protected material entered model context"
        );
    }
    let messages = request.body["messages"].as_array().unwrap();
    let users: Vec<_> = messages
        .iter()
        .filter(|message| message["role"] == "user")
        .map(|message| message["content"].as_str().unwrap())
        // Native boundary notices also project to provider user messages.
        // Actual human-input identity/count is checked in the typed store below.
        .filter(|text| [ROLE_REQUEST, ROLE_CHOICE].contains(text))
        .collect();
    assert_eq!(
        users,
        if phase < 4 {
            vec![ROLE_REQUEST]
        } else {
            vec![ROLE_REQUEST, ROLE_CHOICE]
        }
    );
    if phase >= 4 {
        assert!(
            messages.iter().any(
                |message| message["role"] == "assistant" && message["content"] == ROLE_QUESTION
            )
        );
    }
    let names: Vec<_> = request.body["tools"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|tool| tool["function"]["name"].as_str())
        .collect();
    assert!(names.contains(&"paired_host_select_host"));
    assert!(!names.contains(&"setup_install") && !names.contains(&"setup_catalog"));
    let call = match phase {
        0 => {
            assert!(!names.contains(&"paired_host_frontend_source_inspect"));
            Some(("role-host", "paired_host_select_host", json!({})))
        }
        1 => {
            assert_eq!(
                tool_result(&request.body, "role-host"),
                json!({"state":"selected","changed":true})
            );
            Some((
                "role-inventory",
                "paired_host_frontend_inventory",
                json!({}),
            ))
        }
        2 => {
            assert_eq!(tool_result(&request.body, "role-inventory"), inventory());
            Some((
                "role-inspect",
                "paired_host_frontend_source_inspect",
                json!({"repo":ROLE_REPO,"tag":"v0.1.0","package_path":"extension"}),
            ))
        }
        3 => {
            assert_eq!(
                tool_result(&request.body, "role-inspect"),
                role_inspection()
            );
            None
        }
        4 => {
            // Use the previous native tool result, including its source pins and
            // facade order. The new human input supplies the map and role choice.
            let observed = tool_result(&request.body, "role-inspect");
            assert_eq!(observed, role_inspection());
            Some((
                "role-prepare",
                "paired_host_frontend_prepare",
                json!({"new_extension":combined_role_proposal(&observed)}),
            ))
        }
        5 => {
            let observed = tool_result(&request.body, "role-prepare");
            let mut expected = role_prepared();
            expected.as_object_mut().unwrap().remove("review_url");
            expected["review_presentation"] = json!("queued");
            assert_eq!(observed, expected);
            Some((
                "role-submit",
                "paired_host_frontend_submit",
                json!({"change_digest":observed["change"]["change_digest"]}),
            ))
        }
        6 => {
            let observed = tool_result(&request.body, "role-submit");
            assert_eq!(observed["state"], "submitted");
            assert_eq!(observed["attempt"], 2);
            assert_eq!(observed["http_status"], 201);
            assert_eq!(observed["change"], role_prepared()["change"]);
            assert_eq!(observed["request_id"], observed["change"]["change_digest"]);
            Some((
                "role-status",
                "paired_host_frontend_status",
                json!({"change_digest":observed["request_id"]}),
            ))
        }
        7 => {
            assert_eq!(tool_result(&request.body, "role-status"), role_built());
            None
        }
        _ => panic!("unexpected role provider request or replay"),
    };
    if let Some((id, name, arguments)) = call {
        assert!(
            names.contains(&name),
            "the real native catalog must offer this tool"
        );
        return deferred_call_response(id, name, arguments);
    }
    let event = json!({"choices":[{"delta":{"content":if phase == 3 { ROLE_QUESTION } else { ROLE_ANSWER }},"finish_reason":"stop"}]});
    format!(
        "data: {event}\n\ndata: {{\"choices\":[],\"usage\":{{\"prompt_tokens\":100,\"completion_tokens\":20,\"total_tokens\":120}}}}\n\ndata: [DONE]\n\n"
    )
}

fn model_response(request: &Request, phase: usize) -> String {
    assert_eq!(request.method, "POST");
    assert_eq!(request.path, "/v1/chat/completions");
    assert_eq!(request.headers["authorization"], format!("Bearer {TOKEN}"));
    assert!(!request.headers.contains_key("cookie"));
    assert_eq!(request.body["model"], "cli-fixture-remote");
    assert_eq!(request.body["stream"], true);
    let serialized = request.body.to_string();
    for protected in [CODE, DEVICE, PAIR, TOKEN] {
        assert!(
            !serialized.contains(protected),
            "protected value entered model request"
        );
    }
    let change = prepared()["change"].clone();
    let call = match phase {
        0 => Some(("inventory", "paired_host_frontend_inventory", json!({}))),
        1 => {
            let observed = tool_result(&request.body, "inventory");
            assert_eq!(observed, inventory());
            let extension = &observed["extensions"][0];
            let channel = &extension["channels"][0];
            Some((
                "candidate",
                "paired_host_frontend_prepare",
                json!({
                "extension_id":extension["extension_id"],"channel_id":channel["channel_id"],
                "ingress":channel["ingress"][0],
                "participants":{"approved_groups":channel["approved_groups"]["declared"]}}),
            ))
        }
        2 => {
            let observed = tool_result(&request.body, "candidate");
            let mut expected = prepared();
            expected["review_presentation"] = json!("unavailable");
            assert_eq!(observed, expected);
            Some((
                "submit",
                "paired_host_frontend_submit",
                json!({"change_digest":observed["change"]["change_digest"]}),
            ))
        }
        3 => {
            let observed = tool_result(&request.body, "submit");
            assert_eq!(observed["state"], "submitted");
            assert_eq!(observed["http_status"], 201);
            assert_eq!(observed["change"], change);
            assert_eq!(observed["request_id"], change["change_digest"]);
            Some((
                "status",
                "paired_host_frontend_status",
                json!({"change_digest":observed["request_id"]}),
            ))
        }
        4 => {
            let observed = tool_result(&request.body, "status");
            assert_eq!(observed["state"], "submitted");
            assert_eq!(observed["request_id"], change["change_digest"]);
            assert!(observed.get("activation_id").is_none());
            None
        }
        _ => panic!("unexpected provider request/retry"),
    };
    let event = if let Some((id, name, arguments)) = call {
        assert!(
            request.body["tools"]
                .as_array()
                .unwrap()
                .iter()
                .any(|tool| tool["function"]["name"] == name)
        );
        json!({"choices":[{"delta":{"tool_calls":[{"index":0,"id":id,"type":"function",
            "function":{"name":name,"arguments":arguments.to_string()}}]},"finish_reason":"tool_calls"}]})
    } else {
        json!({"choices":[{"delta":{"content":ANSWER},"finish_reason":"stop"}]})
    };
    format!(
        "data: {event}\n\ndata: {{\"choices\":[],\"usage\":{{\"prompt_tokens\":100,\"completion_tokens\":20,\"total_tokens\":120}}}}\n\ndata: [DONE]\n\n"
    )
}

fn recipe_model_response(request: &Request, phase: usize) -> String {
    use toolkit_package::{LIST_RECIPES, READ_RECIPE};
    assert_eq!(request.method, "POST");
    assert_eq!(request.path, "/v1/chat/completions");
    assert_eq!(request.headers["authorization"], format!("Bearer {TOKEN}"));
    assert_eq!(request.body["model"], "cli-fixture-remote");
    let messages = request.body["messages"].as_array().unwrap();
    assert!(
        messages
            .iter()
            .any(|m| m["role"] == "user" && m["content"] == RECIPE_REQUEST)
    );
    assert!(!request.body.to_string().contains(EXISTING_REALM_HISTORY));
    let tools = request.body["tools"].as_array().unwrap();
    for name in [LIST_RECIPES, READ_RECIPE] {
        assert!(tools.iter().any(|t| t["function"]["name"] == name));
    }
    for name in ["setup_catalog", "setup_install", "find_and_install"] {
        assert!(!tools.iter().any(|t| t["function"]["name"] == name));
    }
    let catalog = toolkit_package::bundled_recipe_catalog().unwrap();
    let call = match phase {
        0 => Some(("recipes-first", LIST_RECIPES, json!({"limit":1}))),
        1 => {
            let page = tool_result(&request.body, "recipes-first");
            let first = &page["entries"][0];
            assert_eq!(page["entries"].as_array().unwrap().len(), 1);
            assert!(
                first["roles"]
                    .as_array()
                    .unwrap()
                    .contains(&json!("service_connector"))
            );
            Some((
                "recipe-exact",
                READ_RECIPE,
                json!({"identity":first["identity"],"digest":first["digest"]}),
            ))
        }
        2 => {
            let page = tool_result(&request.body, "recipes-first");
            let full = tool_result(&request.body, "recipe-exact");
            let identity = serde_json::from_value(page["entries"][0]["identity"].clone()).unwrap();
            assert_eq!(
                full,
                serde_json::to_value(catalog.get(&identity).unwrap()).unwrap()
            );
            assert!(
                !full["package"]["known_exports"]
                    .as_array()
                    .unwrap()
                    .is_empty()
            );
            assert!(full.get("library").is_none());
            Some((
                "recipes-next",
                LIST_RECIPES,
                json!({"limit":1,"snapshot":page["snapshot"],"after":page["next_after"]}),
            ))
        }
        3 => {
            let first = tool_result(&request.body, "recipes-first");
            let next = tool_result(&request.body, "recipes-next");
            assert_eq!(first["snapshot"], next["snapshot"]);
            assert_eq!(
                next["entries"][0]["identity"],
                json!(catalog.entries().nth(1).unwrap().package.identity)
            );
            None
        }
        4 => {
            assert!(
                messages
                    .iter()
                    .any(|m| m["role"] == "user" && m["content"] == RECIPE_FOLLOWUP)
            );
            assert!(
                messages
                    .iter()
                    .any(|m| m["role"] == "assistant" && m["content"] == RECIPE_ANSWER)
            );
            None
        }
        _ => panic!("unexpected recipe provider request or retry"),
    };
    let event = if let Some((id, name, arguments)) = call {
        json!({"choices":[{"delta":{"tool_calls":[{"index":0,"id":id,"type":"function",
            "function":{"name":name,"arguments":arguments.to_string()}}]},"finish_reason":"tool_calls"}]})
    } else {
        let answer = if phase == 3 {
            RECIPE_ANSWER
        } else {
            RECIPE_FOLLOWUP_ANSWER
        };
        json!({"choices":[{"delta":{"content":answer},"finish_reason":"stop"}]})
    };
    format!(
        "data: {event}\n\ndata: {{\"choices\":[],\"usage\":{{\"prompt_tokens\":100,\"completion_tokens\":20,\"total_tokens\":120}}}}\n\ndata: [DONE]\n\n"
    )
}

fn document_refusal_response(request: &Request, phase: usize) -> String {
    assert_eq!(request.method, "POST");
    assert_eq!(request.path, "/v1/chat/completions");
    assert_eq!(request.headers["authorization"], format!("Bearer {TOKEN}"));
    assert_eq!(request.body["model"], "cli-fixture-remote");
    assert!(request.body.to_string().contains(DOCUMENT_REQUEST));
    assert!(
        request.body["tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|tool| tool["function"]["name"] == "read_public_document")
    );
    let event = match phase {
        4 => {
            json!({"choices":[{"delta":{"tool_calls":[{"index":0,"id":"private-document","type":"function",
            "function":{"name":"read_public_document","arguments":json!({"url":DOCUMENT_URL}).to_string()}}]},"finish_reason":"tool_calls"}]})
        }
        5 => {
            let result = tool_result(&request.body, "private-document");
            assert_eq!(result["error"], "destination_refused");
            assert!(result.get("text").is_none());
            json!({"choices":[{"delta":{"content":DOCUMENT_ANSWER},"finish_reason":"stop"}]})
        }
        _ => panic!("unexpected public-document refusal phase"),
    };
    format!("data: {event}\n\ndata: [DONE]\n\n")
}

fn deferred_model_response(request: &Request, phase: usize, mode: DeferredMode) -> String {
    let services = mode.services();
    let prebound = mode.prebound();
    let initial = usize::from(prebound);
    assert_eq!(request.method, "POST");
    assert_eq!(request.path, "/v1/chat/completions");
    assert_eq!(request.headers["authorization"], format!("Bearer {TOKEN}"));
    assert!(!request.headers.contains_key("cookie"));
    let serialized = request.body.to_string();
    for protected in [CODE, DEVICE, PAIR, TOKEN] {
        assert!(
            !serialized.contains(protected),
            "protected value entered deferred model request"
        );
    }
    assert!(
        serialized.contains(mode.prompt()),
        "original input must survive lazy preparation and host selection"
    );
    let names: Vec<_> = request.body["tools"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|tool| tool["function"]["name"].as_str())
        .collect();
    assert_eq!(names.contains(&"paired_host_select_host"), !prebound);
    assert!(names.contains(&"toolkit_prepare_services"));
    if prebound && phase == 0 {
        assert!(names.contains(&"paired_host_frontend_inventory"));
        assert!(!names.contains(&"setup_catalog"));
        return deferred_call_response(
            "early-inventory",
            "paired_host_frontend_inventory",
            json!({}),
        );
    }
    if prebound {
        assert_eq!(tool_result(&request.body, "early-inventory"), inventory());
    }
    if phase == initial {
        assert!(!names.contains(&"setup_help"));
        assert!(!names.contains(&"setup_catalog"));
        assert_eq!(names.contains(&"paired_host_frontend_inventory"), prebound);
        return deferred_call_response("frontends", "toolkit_frontend_catalog", json!({}));
    }
    let catalog = toolkit_package::bundled_discovery_catalog().unwrap();
    assert_eq!(
        tool_result(&request.body, "frontends"),
        serde_json::to_value(toolkit_package::frontend_catalog_observation(&catalog)).unwrap()
    );
    if services && phase == initial + 1 {
        assert!(!names.contains(&"setup_help"));
        return deferred_call_response("activate-services", "toolkit_prepare_services", json!({}));
    }
    let messages = request.body["messages"].as_array().unwrap();
    if services {
        let activation = messages
            .iter()
            .find(|message| message["tool_call_id"] == "activate-services")
            .unwrap();
        let observed: Value =
            serde_json::from_str(activation["content"].as_str().unwrap()).unwrap();
        assert_eq!(observed["state"], "requested");
        assert_eq!(observed["changed"], true);
        assert_eq!(observed["native_pending_sources"], 1);
        assert_eq!(observed.as_object().unwrap().len(), 3);
    } else {
        assert!(!names.contains(&"setup_help"));
        assert!(
            !messages
                .iter()
                .any(|message| message["tool_call_id"] == "activate-services")
        );
    }
    let catalog_results: Vec<_> = messages
        .iter()
        .filter(|message| {
            message["role"] == "tool"
                && message["tool_call_id"]
                    .as_str()
                    .is_some_and(|id| id.starts_with("catalog-"))
        })
        .collect();
    assert!(
        catalog_results.len() <= 12,
        "native source must settle within finite fixture budget"
    );
    if services && !names.contains(&mode.service_tools()[0]) {
        let latest = catalog_results.last();
        let ready = latest.is_some_and(|message| {
            let id = message["tool_call_id"].as_str().unwrap();
            assert!(
                id.starts_with("catalog-search-"),
                "accepted load must expose setup tools on next native request"
            );
            let observed: Value =
                serde_json::from_str(message["content"].as_str().unwrap()).unwrap();
            assert_eq!(observed["catalog_exact"], true);
            if observed["pending_sources"]
                .as_array()
                .is_some_and(|pending| !pending.is_empty())
            {
                assert_eq!(observed["pending_sources"], json!(["toolkit-setup"]));
                return false;
            }
            assert!(observed["empty_result_status"].is_null());
            for name in mode.service_tools() {
                assert!(observed["results"].as_array().unwrap().iter().any(|row| {
                    row["name"] == *name
                        && ["deferred", "loaded"]
                            .contains(&row["visibility_status"].as_str().unwrap())
                }));
            }
            true
        });
        let (id, name, args) = if ready {
            (
                format!("catalog-load-{phase}"),
                "tool_catalog_load",
                json!({"names":mode.service_tools()}),
            )
        } else {
            (
                format!("catalog-search-{phase}"),
                "tool_catalog_search",
                json!({"query":"setup", "limit":50}),
            )
        };
        assert!(
            names.contains(&name),
            "native catalog control {name} is absent; visible tool names: {names:?}"
        );
        return deferred_call_response(&id, name, args);
    }
    if let Some(message) = catalog_results.last()
        && message["tool_call_id"]
            .as_str()
            .unwrap()
            .starts_with("catalog-load-")
    {
        let load: Value = serde_json::from_str(message["content"].as_str().unwrap()).unwrap();
        assert_eq!(load["catalog_exact"], true);
        for name in mode.service_tools() {
            assert!(
                load["resolutions"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|row| row["name"] == *name && row["accepted"] == true)
            );
        }
    }
    let phase = phase.checked_sub(catalog_results.len()).unwrap();
    let frontend_start = mode.frontend_start();
    if phase < frontend_start && !prebound {
        assert!(!names.contains(&"paired_host_frontend_inventory"));
        assert!(!names.contains(&"paired_host_frontend_submit"));
    }
    if (frontend_start..frontend_start + 5).contains(&phase) {
        if !prebound {
            assert_eq!(
                tool_result(&request.body, "select"),
                json!({"state":"selected","changed":true})
            );
        } else {
            assert_idle_library(&tool_result(&request.body, "library-state"));
        }
        assert!(names.contains(&"paired_host_frontend_inventory"));
        return model_response(request, phase - frontend_start);
    }
    let call = match phase {
        phase if phase == initial + 2 && mode.supplied() => {
            Some(("library-catalog", "setup_catalog", json!({})))
        }
        phase if phase == initial + 3 && mode.supplied() => {
            let catalog = tool_result(&request.body, "library-catalog");
            assert_supplied_catalog(&catalog);
            Some((
                "library-entry",
                "setup_catalog_entry",
                catalog["entries"][0]["identity"].clone(),
            ))
        }
        phase if phase == initial + 4 && mode.supplied() => {
            let catalog = tool_result(&request.body, "library-catalog");
            let entry = tool_result(&request.body, "library-entry");
            for key in ["scope", "catalog_sha256", "contents_sha256"] {
                assert_eq!(entry[key], catalog[key]);
            }
            assert_eq!(
                entry["entry"]["identity"],
                catalog["entries"][0]["identity"]
            );
            assert!(entry["entry"]["provenance"].is_object());
            assert!(entry["entry"]["strategies"].is_array());
            Some(("forbidden-install", "setup_install", json!({})))
        }
        phase if phase == initial + 5 && mode.supplied() => {
            assert_install_denied(messages);
            Some(("library-state", "setup_state", json!({})))
        }
        6 if mode == DeferredMode::SuppliedLibrary => {
            assert_idle_library(&tool_result(&request.body, "library-state"));
            Some(("select", "paired_host_select_host", json!({})))
        }
        2 if services => Some(("help", "setup_help", json!({}))),
        3 if services => {
            let messages = request.body["messages"].as_array().unwrap();
            let observed = messages
                .iter()
                .find(|message| message["tool_call_id"] == "help")
                .unwrap();
            assert!(
                observed["content"]
                    .as_str()
                    .unwrap()
                    .contains("service-preparation-only")
            );
            Some(("forbidden-install", "setup_install", json!({})))
        }
        4 if services => {
            assert_install_denied(messages);
            Some(("select", "paired_host_select_host", json!({})))
        }
        1 if !services => Some(("select", "paired_host_select_host", json!({}))),
        phase if phase == frontend_start + 5 => {
            assert!(serialized.contains(ANSWER) && serialized.contains(FOLLOWUP));
            if prebound {
                Some((
                    "followup-inventory",
                    "paired_host_frontend_inventory",
                    json!({}),
                ))
            } else {
                Some(("same-host", "paired_host_select_host", json!({})))
            }
        }
        phase if phase == frontend_start + 6 && !prebound => {
            assert_eq!(
                tool_result(&request.body, "same-host"),
                json!({"state":"selected","changed":false})
            );
            Some((
                "followup-inventory",
                "paired_host_frontend_inventory",
                json!({}),
            ))
        }
        phase if phase == frontend_start + 7 - usize::from(prebound) => {
            assert_eq!(
                tool_result(&request.body, "followup-inventory"),
                inventory()
            );
            None
        }
        _ => panic!("unexpected deferred provider request or replay"),
    };
    let event = if let Some((id, name, arguments)) = call {
        if name != "setup_install" {
            assert!(
                names.contains(&name),
                "tool must be present in actual native catalog"
            );
        }
        return deferred_call_response(id, name, arguments);
    } else {
        json!({"choices":[{"delta":{"content":FOLLOWUP_ANSWER},"finish_reason":"stop"}]})
    };
    format!(
        "data: {event}\n\ndata: {{\"choices\":[],\"usage\":{{\"prompt_tokens\":100,\"completion_tokens\":20,\"total_tokens\":120}}}}\n\ndata: [DONE]\n\n"
    )
}

fn assert_install_denied(messages: &[Value]) {
    let observed = messages
        .iter()
        .find(|message| message["role"] == "tool" && message["tool_call_id"] == "forbidden-install")
        .unwrap();
    assert!(
        observed["content"]
            .as_str()
            .unwrap()
            .contains("access_denied"),
        "native installation denial required, not argument or server rejection"
    );
}

fn assert_supplied_catalog(catalog: &Value) {
    assert_eq!(catalog["scope"], "catalog_metadata_only");
    for key in ["catalog_sha256", "contents_sha256"] {
        let digest = catalog[key].as_str().unwrap();
        assert!(digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit()));
    }
    let rows = catalog["entries"].as_array().unwrap();
    assert!(
        (100..=512).contains(&rows.len()),
        "100-plus catalog metadata must reach the native model"
    );
    let mut identities = BTreeSet::new();
    for row in rows {
        let identity = row["identity"].as_object().unwrap();
        assert_eq!(identity.len(), 3);
        let key: Vec<_> = ["publisher", "name", "version"]
            .map(|key| {
                let value = identity[key].as_str().unwrap();
                assert!(!value.is_empty() && value.len() <= 256);
                value.to_owned()
            })
            .into_iter()
            .collect();
        assert!(
            identities.insert(key),
            "catalog identities must be distinct"
        );
    }
}

fn assert_idle_library(state: &Value) {
    assert!(state["job"].is_null());
    assert_eq!(state["job_history"]["count"], 0);
    assert_eq!(state["operation_in_progress"], false);
    let families = state["families"].as_array().unwrap();
    assert!(!families.is_empty());
    for family in families {
        assert_eq!(family["status"], "unprepared");
        assert_eq!(family["profiles"], json!([]));
    }
}

fn deferred_call_response(id: &str, name: &str, arguments: Value) -> String {
    let event = json!({"choices":[{"delta":{"tool_calls":[{"index":0,"id":id,"type":"function",
        "function":{"name":name,"arguments":arguments.to_string()}}]},"finish_reason":"tool_calls"}]});
    format!(
        "data: {event}\n\ndata: {{\"choices\":[],\"usage\":{{\"prompt_tokens\":100,\"completion_tokens\":20,\"total_tokens\":120}}}}\n\ndata: [DONE]\n\n"
    )
}

fn secret_inventory() -> Value {
    let mut observed = inventory();
    observed["extensions"][0]["channels"][0]["secrets"] = json!([
        {"name":"telegram_bot_token","source":"vault","present":false,
            "generation":null,"binding_status":null}
    ]);
    observed
}

fn secret_prepared() -> Value {
    let mut observed = prepared();
    observed["missing_prerequisites"][0]["name"] = json!("telegram_bot_token");
    let change = &mut observed["change"];
    for version in ["before", "after"] {
        change["ledger_patch"][version]["secret_names"] = json!(["telegram_bot_token"]);
    }
    change.as_object_mut().unwrap().remove("change_digest");
    change["change_digest"] = json!(format!(
        "paired-host-change-v1:sha256:{:x}",
        Sha256::digest(serde_json::to_vec(change).unwrap())
    ));
    observed["review_url"] = json!(format!(
        "/console/extensions/frontend/review/{}",
        observed["change"]["change_digest"].as_str().unwrap()
    ));
    observed
}

fn secret_request() -> Value {
    json!({"extension_id":"telegram","binding_name":"telegram_bot_token",
        "base":secret_prepared()["change"]["base"]})
}

fn secret_observation(state: &str) -> Value {
    let mut observed = json!({"intent_id":INTENT_ID,"state":state,
        "target":{"host":"fixture-owner-label.invalid","activation_id":"210",
            "extension_id":"telegram","binding_name":"telegram_bot_token",
            "channel_ids":["telegram:primary"],"consumer":"channel:telegram:primary",
            "consumer_verified":true,"required":true},
        "present":false,"requires_overwrite":false,"presenter":"terminal",
        "entry_url":ENTRY_PATH});
    if state == "committed" {
        // The fixture supplies a later host observation. Connect never enters
        // a value or confirms an intent to manufacture this state transition.
        observed["fingerprint"] = json!("1a2b3c4d");
        observed["overwrite"] = json!(false);
        observed["generation"] = json!(1);
        observed["binding_status"] = json!("committed_not_live");
    }
    observed
}

fn public_secret_observation(state: &str) -> Value {
    let mut observed = secret_observation(state);
    let fields = observed.as_object_mut().unwrap();
    fields.remove("entry_url");
    for name in [
        "fingerprint",
        "overwrite",
        "generation",
        "binding_status",
        "reason",
    ] {
        fields.entry(name).or_insert(Value::Null);
    }
    observed
}

fn secret_model_response(request: &Request, phase: usize) -> String {
    assert_eq!(request.method, "POST");
    assert_eq!(request.path, "/v1/chat/completions");
    assert_eq!(request.headers["authorization"], format!("Bearer {TOKEN}"));
    assert!(!request.headers.contains_key("cookie"));
    assert_eq!(request.body["model"], "cli-fixture-remote");
    assert_eq!(request.body["stream"], true);
    let serialized = request.body.to_string();
    assert!(!serialized.contains("/console/extensions/frontend/review/"));
    for protected in [
        CODE,
        DEVICE,
        PAIR,
        TOKEN,
        ENTRY_PATH,
        "entry_url",
        "entry_path",
    ] {
        assert!(
            !serialized.contains(protected),
            "private entry material entered the model request"
        );
    }
    let call = match phase {
        0 => Some(("inventory", "paired_host_frontend_inventory", json!({}))),
        1 => {
            let observed = tool_result(&request.body, "inventory");
            assert_eq!(observed, secret_inventory());
            let extension = &observed["extensions"][0];
            let channel = &extension["channels"][0];
            Some((
                "candidate",
                "paired_host_frontend_prepare",
                json!({
                "extension_id":extension["extension_id"],"channel_id":channel["channel_id"],
                "ingress":channel["ingress"][0],
                "participants":{"approved_groups":channel["approved_groups"]["declared"]}}),
            ))
        }
        2 => {
            let observed = tool_result(&request.body, "candidate");
            let mut expected = secret_prepared();
            expected.as_object_mut().unwrap().remove("review_url");
            expected["review_presentation"] = json!("queued");
            assert_eq!(observed, expected);
            let selected = tool_result(&request.body, "inventory");
            let extension = &selected["extensions"][0];
            let binding = &extension["channels"][0]["secrets"][0];
            assert_eq!(binding["source"], "vault");
            assert_eq!(binding["present"], false);
            assert_eq!(
                binding["name"],
                observed["missing_prerequisites"][0]["name"]
            );
            let arguments = json!({"extension_id":extension["extension_id"],
                "binding_name":binding["name"],"base":observed["change"]["base"]});
            assert_eq!(arguments, secret_request());
            Some((
                "secret-create",
                "paired_host_extension_secret_create",
                arguments,
            ))
        }
        3 => {
            let observed = tool_result(&request.body, "secret-create");
            assert_eq!(
                observed,
                json!({"outcome":"created","result":{
                "request":secret_request(),"intent":public_secret_observation("awaiting_entry")},
                "entry_presentation":"queued"})
            );
            Some((
                "secret-pending",
                "paired_host_extension_secret_status",
                json!({"intent_id":observed["result"]["intent"]["intent_id"]}),
            ))
        }
        4 => {
            let observed = tool_result(&request.body, "secret-pending");
            assert_eq!(
                observed,
                json!({"outcome":"status",
                "result":public_secret_observation("awaiting_entry")})
            );
            Some((
                "secret-committed",
                "paired_host_extension_secret_status",
                json!({"intent_id":observed["result"]["intent_id"]}),
            ))
        }
        5 => {
            let observed = tool_result(&request.body, "secret-committed");
            assert_eq!(
                observed,
                json!({"outcome":"status",
                "result":public_secret_observation("committed")})
            );
            None
        }
        6 => {
            assert!(serialized.contains(REVIEW_REQUEST));
            Some((
                "review-again",
                "paired_host_frontend_status",
                json!({"change_digest":secret_prepared()["change"]["change_digest"],"show_review":true}),
            ))
        }
        7 => {
            assert_eq!(
                tool_result(&request.body, "review-again"),
                json!({
                    "state":"prepared", "change":secret_prepared()["change"],
                    "base_current":false, "review_presentation":"queued",
                })
            );
            None
        }
        _ => panic!("unexpected secret provider request/retry"),
    };
    let event = if let Some((id, name, arguments)) = call {
        assert!(
            request.body["tools"]
                .as_array()
                .unwrap()
                .iter()
                .any(|tool| tool["function"]["name"] == name)
        );
        json!({"choices":[{"delta":{"tool_calls":[{"index":0,"id":id,"type":"function",
            "function":{"name":name,"arguments":arguments.to_string()}}]},"finish_reason":"tool_calls"}]})
    } else {
        json!({"choices":[{"delta":{"content":if phase == 7 { REVIEW_ANSWER } else { SECRET_ANSWER }},"finish_reason":"stop"}]})
    };
    format!(
        "data: {event}\n\ndata: {{\"choices\":[],\"usage\":{{\"prompt_tokens\":100,\"completion_tokens\":20,\"total_tokens\":120}}}}\n\ndata: [DONE]\n\n"
    )
}

fn recovery_model_response(request: &Request, phase: usize) -> String {
    // The first three ordinary calls use the same inventory/preparation and
    // protected approval path as the successful executable scenario.
    if phase < 3 {
        return model_response(request, phase);
    }
    assert_eq!(request.method, "POST");
    assert_eq!(request.path, "/v1/chat/completions");
    assert_eq!(request.headers["authorization"], format!("Bearer {TOKEN}"));
    assert!(!request.headers.contains_key("cookie"));
    assert_eq!(request.body["model"], "cli-fixture-remote");
    assert_eq!(request.body["stream"], true);
    let visible = request.body.to_string();
    for protected in [CODE, DEVICE, PAIR, TOKEN] {
        assert!(!visible.contains(protected));
    }
    assert!(visible.contains(RECOVERY_REQUEST));
    let change = prepared()["change"].clone();
    let call = match phase {
        3 => {
            let observed = tool_result(&request.body, "submit");
            assert_eq!(observed["error"], "execution_failed");
            assert!(observed.to_string().contains("unconfirmed"));
            assert!(!visible.contains(STATUS_REQUEST));
            // A stale/unavailable tool name causes an actual native fatal
            // precheck after the uncertain effect, not a synthetic completion.
            Some(("stale-tool", "fixture_unavailable_tool", json!({})))
        }
        4 => {
            assert!(visible.contains(STATUS_REQUEST), "no automatic status turn");
            Some((
                "exact-status",
                "paired_host_frontend_status",
                json!({"change_digest":change["change_digest"]}),
            ))
        }
        5 => {
            let observed = tool_result(&request.body, "exact-status");
            assert_eq!(observed["state"], "submitted");
            assert_eq!(observed["request_id"], change["change_digest"]);
            Some((
                "unrelated-status",
                "paired_host_frontend_status",
                json!({
                    "change_digest":format!("paired-host-change-v1:sha256:{}", "a".repeat(64))
                }),
            ))
        }
        6 => {
            assert_eq!(
                tool_result(&request.body, "unrelated-status")["error"],
                "access_denied"
            );
            Some((
                "repeat-submit",
                "paired_host_frontend_submit",
                json!({"change_digest":change["change_digest"]}),
            ))
        }
        7 => {
            assert_eq!(
                tool_result(&request.body, "repeat-submit")["error"],
                "access_denied"
            );
            Some((
                "uncertain-recipes",
                "list_integration_recipes",
                json!({"limit":1}),
            ))
        }
        8 => {
            assert_eq!(
                tool_result(&request.body, "uncertain-recipes")["error"],
                "access_denied"
            );
            let catalog = toolkit_package::bundled_recipe_catalog().unwrap();
            let entry = catalog.entries().next().unwrap();
            Some((
                "uncertain-recipe",
                "read_integration_recipe",
                json!({"identity":entry.package.identity,"digest":entry.digest}),
            ))
        }
        9 => {
            assert_eq!(
                tool_result(&request.body, "uncertain-recipe")["error"],
                "access_denied"
            );
            Some((
                "uncertain-review",
                "paired_host_frontend_status",
                json!({"change_digest":change["change_digest"],"show_review":true}),
            ))
        }
        10 => {
            assert_eq!(
                tool_result(&request.body, "uncertain-review")["error"],
                "access_denied"
            );
            Some((
                "uncertain-document",
                "read_public_document",
                json!({"url":DOCUMENT_URL}),
            ))
        }
        11 => {
            assert_eq!(
                tool_result(&request.body, "uncertain-document")["error"],
                "access_denied"
            );
            None
        }
        _ => panic!("unexpected provider request or native replay"),
    };
    let event = if let Some((id, name, arguments)) = call {
        let offered = request.body["tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|tool| tool["function"]["name"] == name);
        assert_eq!(
            offered,
            phase != 3,
            "only the deliberate fatal name is absent"
        );
        json!({"choices":[{"delta":{"tool_calls":[{"index":0,"id":id,"type":"function",
            "function":{"name":name,"arguments":arguments.to_string()}}]},"finish_reason":"tool_calls"}]})
    } else {
        json!({"choices":[{"delta":{"content":STATUS_ANSWER},"finish_reason":"stop"}]})
    };
    format!(
        "data: {event}\n\ndata: {{\"choices\":[],\"usage\":{{\"prompt_tokens\":100,\"completion_tokens\":20,\"total_tokens\":120}}}}\n\ndata: [DONE]\n\n"
    )
}

struct Server {
    origin: String,
    requests: Arc<Mutex<Vec<Request>>>,
    stop: Option<oneshot::Sender<()>>,
    task: Option<tokio::task::JoinHandle<()>>,
}

impl Server {
    async fn start(provider: bool) -> Self {
        Self::start_scenario(provider, false).await
    }

    async fn start_scenario(provider: bool, secret_entry: bool) -> Self {
        Self::start_options(provider, secret_entry, false).await
    }

    async fn start_options(provider: bool, secret_entry: bool, deferred: bool) -> Self {
        Self::start_mode(
            provider,
            secret_entry,
            deferred.then_some(DeferredMode::SyntheticServices),
        )
        .await
    }

    async fn start_frontend_only() -> Self {
        Self::start_mode(true, false, Some(DeferredMode::FrontendOnly)).await
    }

    async fn start_mode(
        provider: bool,
        secret_entry: bool,
        deferred: Option<DeferredMode>,
    ) -> Self {
        Self::start_case(
            provider,
            secret_entry,
            deferred,
            false,
            false,
            false,
            false,
            false,
        )
        .await
    }

    async fn start_recovery(provider: bool) -> Self {
        Self::start_case(provider, false, None, true, false, false, false, false).await
    }

    async fn start_recipe_discovery() -> Self {
        Self::start_case(true, false, None, false, true, false, false, false).await
    }

    async fn start_recipe_failure() -> Self {
        Self::start_case(true, false, None, false, true, true, false, false).await
    }

    async fn start_document_refusal() -> Self {
        Self::start_case(true, false, None, false, true, false, true, false).await
    }

    async fn start_roles(provider: bool) -> Self {
        Self::start_case(provider, false, None, false, false, false, false, true).await
    }

    async fn start_case(
        provider: bool,
        secret_entry: bool,
        deferred: Option<DeferredMode>,
        recovery: bool,
        recipes: bool,
        recipe_failure: bool,
        document_refusal: bool,
        roles: bool,
    ) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let (stop, mut stopped) = oneshot::channel();
        let requests = Arc::new(Mutex::new(Vec::<Request>::new()));
        let captured = requests.clone();
        let task = tokio::spawn(async move {
            loop {
                let mut stream = tokio::select! {
                    biased;
                    _ = &mut stopped => break,
                    connection = listener.accept() => connection.unwrap().0,
                };
                let received = timeout(BOUND, request(&mut stream)).await.unwrap();
                if provider && deferred.is_some_and(DeferredMode::services) {
                    // Synthetic provider latency permits actual asynchronous MCP
                    // readiness; catalog observations still decide search/load.
                    // The real controller also copies its pinned metadata/files
                    // during startup. Keep the same finite observation count.
                    sleep(if deferred.is_some_and(DeferredMode::supplied) {
                        Duration::from_secs(1)
                    } else {
                        Duration::from_millis(100)
                    })
                    .await;
                }
                let phase = captured.lock().unwrap().len();
                let (status, headers, content_type, body) = if recipe_failure && phase == 4 {
                    assert!(provider && recipes);
                    assert_eq!(received.method, "POST");
                    assert_eq!(received.path, "/v1/chat/completions");
                    assert_eq!(received.headers["authorization"], format!("Bearer {TOKEN}"));
                    assert_eq!(received.body["model"], "cli-fixture-remote");
                    let messages = received.body["messages"].as_array().unwrap();
                    assert!(messages.iter().any(|message| {
                        message["role"] == "assistant" && message["content"] == RECIPE_ANSWER
                    }));
                    let latest_input = messages
                        .iter()
                        .rev()
                        .find(|message| message["role"] == "user")
                        .unwrap();
                    assert_eq!(latest_input["content"], RECIPE_FAILED_REQUEST);
                    (
                        400,
                        String::new(),
                        "application/json",
                        json!({"error":{"type":"invalid_request_error","message":RECIPE_PROVIDER_FAILURE}}).to_string(),
                    )
                } else if provider {
                    (
                        200,
                        String::new(),
                        "text/event-stream",
                        if document_refusal && matches!(phase, 4 | 5) {
                            document_refusal_response(&received, phase)
                        } else if recipes {
                            // The failed request occupies one HTTP exchange,
                            // never another recipe or follow-up phase.
                            recipe_model_response(
                                &received,
                                phase
                                    - usize::from(recipe_failure && phase > 4)
                                    - 2 * usize::from(document_refusal && phase > 5),
                            )
                        } else if roles {
                            role_model_response(&received, phase)
                        } else if recovery {
                            recovery_model_response(&received, captured.lock().unwrap().len())
                        } else if let Some(services) = deferred {
                            deferred_model_response(
                                &received,
                                captured.lock().unwrap().len(),
                                services,
                            )
                        } else if secret_entry {
                            secret_model_response(&received, captured.lock().unwrap().len())
                        } else {
                            model_response(&received, captured.lock().unwrap().len())
                        },
                    )
                } else {
                    assert!(!received.headers.contains_key("authorization"));
                    let change = if roles {
                        role_prepared()["change"].clone()
                    } else {
                        prepared()["change"].clone()
                    };
                    let (status, headers, value) = match (
                        received.method.as_str(),
                        received.path.as_str(),
                    ) {
                        ("POST", "/console/pair/request") => {
                            assert_eq!(received.body, json!({"label":"Meerkat Connect CLI"}));
                            (
                                200,
                                format!(
                                    "Set-Cookie: paired_host_pair={PAIR}; Path=/console/pair; HttpOnly; SameSite=Strict\r\n"
                                ),
                                json!({"ok":true}),
                            )
                        }
                        ("POST", "/console/pair/confirm") => {
                            assert_eq!(received.body, json!({"code":CODE}));
                            assert_eq!(
                                received.headers["cookie"],
                                format!("paired_host_pair={PAIR}")
                            );
                            (
                                200,
                                format!(
                                    "Set-Cookie: paired_host_device={DEVICE}; Path=/; HttpOnly; SameSite=Lax\r\nSet-Cookie: paired_host_pair=; Max-Age=0; Path=/console/pair\r\n"
                                ),
                                json!({"ok":true}),
                            )
                        }
                        ("GET", "/console/extensions/frontend/inventory") => (
                            200,
                            String::new(),
                            if secret_entry {
                                secret_inventory()
                            } else {
                                inventory()
                            },
                        ),
                        ("GET", path)
                            if roles
                                && path.starts_with(
                                    "/console/extensions/frontend/sources/inspect?",
                                ) =>
                        {
                            let url = reqwest::Url::parse(&format!("http://fixture.invalid{path}"))
                                .unwrap();
                            let query: Vec<_> = url
                                .query_pairs()
                                .map(|(key, value)| (key.into_owned(), value.into_owned()))
                                .collect();
                            assert_eq!(
                                query,
                                vec![
                                    ("repo".into(), ROLE_REPO.into()),
                                    ("tag".into(), "v0.1.0".into()),
                                    ("package_path".into(), "extension".into())
                                ]
                            );
                            assert!(received.body.is_null());
                            (200, String::new(), role_inspection())
                        }
                        ("POST", "/console/extensions/frontend/prepare") => {
                            assert_eq!(
                                received.body,
                                if roles {
                                    json!({"new_extension":combined_role_proposal(&role_inspection())})
                                } else {
                                    candidate()
                                }
                            );
                            (
                                200,
                                String::new(),
                                if roles {
                                    role_prepared()
                                } else if secret_entry {
                                    secret_prepared()
                                } else {
                                    prepared()
                                },
                            )
                        }
                        ("POST", INTENTS) if secret_entry => {
                            assert_eq!(received.body, secret_request());
                            assert!(
                                !captured
                                    .lock()
                                    .unwrap()
                                    .iter()
                                    .any(|request| request.method == "POST"
                                        && request.path == INTENTS)
                            );
                            (201, String::new(), secret_observation("awaiting_entry"))
                        }
                        ("GET", path)
                            if secret_entry && path == format!("{INTENTS}/{INTENT_ID}") =>
                        {
                            assert!(received.body.is_null());
                            let prior_reads = captured
                                .lock()
                                .unwrap()
                                .iter()
                                .filter(|request| request.method == "GET" && request.path == path)
                                .count();
                            let state = match prior_reads {
                                0 => "awaiting_entry",
                                1 => "committed",
                                _ => panic!("unexpected secret status retry"),
                            };
                            (200, String::new(), secret_observation(state))
                        }
                        ("POST", "/console/extensions/frontend/change-requests") => {
                            assert_eq!(
                                received.body,
                                json!({"change_digest":change["change_digest"],"change":change})
                            );
                            (if recovery { 0 } else { 201 }, String::new(), {
                                let mut receipt = json!({"request_id":change["change_digest"],"state":"submitted",
                                        "submitted_at":"2026-10-07T00:00:00+00:00","submitted_by":"paired-device","change":change});
                                if roles {
                                    receipt["attempt"] = json!(2);
                                }
                                receipt
                            })
                        }
                        ("GET", path)
                            if secret_entry
                                && path
                                    == format!(
                                        "/console/extensions/frontend/change-requests/{}",
                                        secret_prepared()["change"]["change_digest"]
                                            .as_str()
                                            .unwrap()
                                    ) =>
                        {
                            assert!(received.body.is_null());
                            (
                                200,
                                String::new(),
                                json!({"state":"prepared",
                                "change":secret_prepared()["change"], "base_current":false}),
                            )
                        }
                        ("GET", path)
                            if path
                                == format!(
                                    "/console/extensions/frontend/change-requests/{}",
                                    change["change_digest"].as_str().unwrap()
                                ) =>
                        {
                            (
                                200,
                                String::new(),
                                if roles {
                                    role_built()
                                } else {
                                    json!({"request_id":change["change_digest"],"state":"submitted","submitted_at":"2026-10-07T00:00:00+00:00"})
                                },
                            )
                        }
                        _ => panic!("unexpected paired host route or retry"),
                    };
                    if received.path.starts_with("/console/extensions/") {
                        assert_eq!(
                            received.headers["cookie"],
                            format!("paired_host_device={DEVICE}")
                        );
                    }
                    (status, headers, "application/json", value.to_string())
                };
                captured.lock().unwrap().push(received);
                if status == 0 {
                    // The exact POST was received, but no HTTP response is
                    // delivered. Connect cannot infer that it was uncommitted.
                    stream.shutdown().await.unwrap();
                    continue;
                }
                let response = format!(
                    "HTTP/1.1 {status} Fixture\r\nConnection: close\r\nContent-Type: {content_type}\r\nCache-Control: no-store\r\n{headers}Content-Length: {}\r\n\r\n{body}",
                    body.len()
                );
                timeout(BOUND, async {
                    stream.write_all(response.as_bytes()).await.unwrap();
                    stream.shutdown().await.unwrap();
                })
                .await
                .unwrap();
            }
        });
        Self {
            origin,
            requests,
            stop: Some(stop),
            task: Some(task),
        }
    }

    fn submissions(&self) -> usize {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .filter(|request| {
                request.method == "POST"
                    && request.path == "/console/extensions/frontend/change-requests"
            })
            .count()
    }

    async fn finish(&mut self) {
        let _ = self.stop.take().unwrap().send(());
        timeout(BOUND, self.task.take().unwrap())
            .await
            .unwrap()
            .unwrap();
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        if let Some(task) = &self.task {
            task.abort();
        }
    }
}

/// Actual native OAuth state, callback and token publication are exercised with
/// isolated local endpoints. No authorization URL is opened in a real browser.
struct OAuthFixture {
    managed_api_key: bool,
    origin: String,
    requests: Arc<Mutex<Vec<Request>>>,
    release_exchange: Arc<tokio::sync::Semaphore>,
    stop: Option<oneshot::Sender<()>>,
    task: Option<tokio::task::JoinHandle<()>>,
}

impl OAuthFixture {
    async fn start(hold_exchange: bool, fail_exchange: bool) -> Self {
        Self::start_with_grant(hold_exchange, fail_exchange, false).await
    }

    async fn start_login_then_refresh(fail_exchange: bool) -> Self {
        Self::start_with_grant(true, fail_exchange, true).await
    }

    async fn start_with_grant(hold_exchange: bool, fail_exchange: bool, refresh: bool) -> Self {
        Self::start_with_mode(hold_exchange, fail_exchange, refresh, false).await
    }

    async fn start_managed_api_key() -> Self {
        Self::start_with_mode(false, false, false, true).await
    }

    async fn start_with_mode(
        hold_exchange: bool,
        fail_exchange: bool,
        refresh: bool,
        managed_api_key: bool,
    ) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::<Request>::new()));
        let captured = requests.clone();
        let release_exchange = Arc::new(tokio::sync::Semaphore::new(usize::from(!hold_exchange)));
        let release = release_exchange.clone();
        let (stop, mut stopped) = oneshot::channel();
        let task = tokio::spawn(async move {
            loop {
                let (mut stream, _) = tokio::select! {
                    _ = &mut stopped => break,
                    result = listener.accept() => result.unwrap(),
                };
                let received = timeout(BOUND, request(&mut stream)).await.unwrap();
                assert_eq!(received.method, "POST");
                let (status, content_type, body) = match received.path.as_str() {
                    "/anthropic/token" => {
                        assert!(
                            !managed_api_key,
                            "API-key entry must not exchange an OAuth token"
                        );
                        let refreshing = received.body["grant_type"] == "refresh_token";
                        if refresh {
                            assert_eq!(
                                captured.lock().unwrap().len(),
                                usize::from(refreshing),
                                "one authorization-code exchange before one native refresh"
                            );
                        }
                        if refresh && refreshing {
                            assert_eq!(received.body["grant_type"], "refresh_token");
                            assert!(received.body["refresh_token"] == OAUTH_REFRESH);
                            assert!(received.body.get("code").is_none());
                            assert!(received.body.get("code_verifier").is_none());
                            assert!(received.body.get("state").is_none());
                        } else {
                            assert_eq!(received.body["grant_type"], "authorization_code");
                            assert_eq!(received.body["code"], OAUTH_CODE);
                            assert!(received.body["code_verifier"].as_str().unwrap().len() >= 43);
                            assert!(!received.body["state"].as_str().unwrap().is_empty());
                        }
                        assert!(!received.headers.contains_key("cookie"));
                        captured.lock().unwrap().push(received);
                        if !refresh || refreshing {
                            let permit = tokio::select! {
                                _ = &mut stopped => break,
                                result = release.acquire() => result.unwrap(),
                            };
                            permit.forget();
                        }
                        if fail_exchange && (!refresh || refreshing) {
                            (
                                500,
                                "application/json",
                                json!({"error":"invalid_grant",
                                "error_description":OAUTH_TOKEN,"private":OAUTH_REFRESH})
                                .to_string(),
                            )
                        } else {
                            // Native login publishes the credential and its durable
                            // lifecycle marker. Issuer expiry inside the native
                            // refresh window makes subsequent materialization refresh.
                            let expires_in = if refresh && !refreshing {
                                meerkat_core::handles::AUTH_LEASE_TTL_REFRESH_WINDOW_SECS / 2
                            } else {
                                3600
                            };
                            (
                                200,
                                "application/json",
                                json!({"access_token":OAUTH_TOKEN,
                            "refresh_token":OAUTH_REFRESH,"token_type":"Bearer",
                            "expires_in":expires_in,"scope":"user:inference user:profile"})
                                .to_string(),
                            )
                        }
                    }
                    "/v1/messages" => {
                        assert!(!refresh, "cancelled startup must not issue a model request");
                        if managed_api_key {
                            assert!(
                                received
                                    .headers
                                    .get("x-api-key")
                                    .is_some_and(|value| value == OAUTH_TOKEN)
                            );
                            assert!(!received.headers.contains_key("authorization"));
                        } else {
                            assert_eq!(
                                received.headers["authorization"],
                                format!("Bearer {OAUTH_TOKEN}")
                            );
                        }
                        assert_eq!(received.body["model"], OAUTH_MODEL);
                        let body = received.body.to_string();
                        assert!(body.contains(OAUTH_PROMPT));
                        for value in [OAUTH_CODE, OAUTH_TOKEN, OAUTH_REFRESH, CODE, DEVICE, PAIR] {
                            assert!(!body.contains(value), "private value entered model request");
                        }
                        for previous in captured.lock().unwrap().iter() {
                            if let Some(state) = previous.body["state"].as_str() {
                                assert!(!body.contains(state), "OAuth state entered model request");
                            }
                        }
                        captured.lock().unwrap().push(received);
                        let events = [
                            (
                                "message_start",
                                json!({"type":"message_start","message":{
                                "id":"msg_fixture","type":"message","role":"assistant",
                                "model":OAUTH_MODEL,"content":[],"stop_reason":null,"stop_sequence":null,
                                "usage":{"input_tokens":100,"output_tokens":0}}}),
                            ),
                            (
                                "content_block_start",
                                json!({"type":"content_block_start","index":0,
                                "content_block":{"type":"text","text":""}}),
                            ),
                            (
                                "content_block_delta",
                                json!({"type":"content_block_delta","index":0,
                                "delta":{"type":"text_delta","text":OAUTH_ANSWER}}),
                            ),
                            (
                                "content_block_stop",
                                json!({"type":"content_block_stop","index":0}),
                            ),
                            (
                                "message_delta",
                                json!({"type":"message_delta",
                                "delta":{"stop_reason":"end_turn","stop_sequence":null},
                                "usage":{"output_tokens":20}}),
                            ),
                            ("message_stop", json!({"type":"message_stop"})),
                        ];
                        (
                            200,
                            "text/event-stream",
                            events
                                .into_iter()
                                .map(|(kind, value)| format!("event: {kind}\ndata: {value}\n\n"))
                                .collect::<String>(),
                        )
                    }
                    _ => panic!("unexpected synthetic OAuth/model endpoint"),
                };
                let response = format!(
                    "HTTP/1.1 {status} Fixture\r\nConnection: close\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\n\r\n{body}",
                    body.len()
                );
                timeout(BOUND, async {
                    stream.write_all(response.as_bytes()).await.unwrap();
                    stream.shutdown().await.unwrap();
                })
                .await
                .unwrap();
            }
        });
        Self {
            managed_api_key,
            origin,
            requests,
            release_exchange,
            stop: Some(stop),
            task: Some(task),
        }
    }

    async fn exchange_received(&self) {
        self.grant_received("authorization_code").await;
    }

    async fn grant_received(&self, grant_type: &str) {
        timeout(BOUND, async {
            loop {
                if self.requests.lock().unwrap().iter().any(|request| {
                    request.path == "/anthropic/token" && request.body["grant_type"] == grant_type
                }) {
                    break;
                }
                sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("synthetic {grant_type} token exchange was not observed"));
    }

    async fn finish(&mut self) {
        let _ = self.stop.take().unwrap().send(());
        timeout(BOUND, self.task.take().unwrap())
            .await
            .unwrap()
            .unwrap();
    }
}

impl Drop for OAuthFixture {
    fn drop(&mut self) {
        if let Some(task) = &self.task {
            task.abort();
        }
    }
}

#[derive(Clone, Copy)]
enum DeviceScenario {
    Progress,
    Wait,
    HeldReady,
}

struct DeviceFixture {
    origin: String,
    calls: Arc<Mutex<Vec<(String, Instant)>>>,
    release: Arc<tokio::sync::Semaphore>,
    stop: Option<oneshot::Sender<()>>,
    task: Option<tokio::task::JoinHandle<()>>,
}

impl DeviceFixture {
    async fn start(scenario: DeviceScenario) -> Self {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let calls = Arc::new(Mutex::new(Vec::new()));
        let captured = calls.clone();
        let release = Arc::new(tokio::sync::Semaphore::new(0));
        let released = release.clone();
        let active = Arc::new(AtomicUsize::new(0));
        let verification_uri = format!("{origin}/human-device-verification");
        let app = axum::Router::new().fallback(
            move |method: axum::http::Method,
                  uri: axum::http::Uri,
                  headers: axum::http::HeaderMap,
                  axum::Form(form): axum::Form<BTreeMap<String, String>>| {
                let captured = captured.clone();
                let released = released.clone();
                let active = active.clone();
                let verification_uri = verification_uri.clone();
                async move {
                    assert_eq!(active.fetch_add(1, Ordering::SeqCst), 0, "native device requests must not overlap");
                    assert_eq!(method, axum::http::Method::POST);
                    assert!(uri.query().is_none());
                    assert!(!headers.contains_key("cookie"));
                    assert!(!headers.contains_key("authorization"));
                    assert_eq!(form["client_id"], meerkat_auth_core::github_copilot::GITHUB_COPILOT_CLIENT_ID);
                    let index = {
                        let mut calls = captured.lock().unwrap();
                        calls.push((uri.path().to_owned(), Instant::now()));
                        calls.len() - 1
                    };
                    let body = match uri.path() {
                        "/copilot/device/code" => {
                            assert_eq!(index, 0, "one native device start only");
                            assert_eq!(form.len(), 2);
                            assert_eq!(form["scope"], "read:user");
                            json!({"device_code":COPILOT_DEVICE_CODE,"user_code":COPILOT_USER_CODE,
                                "verification_uri":verification_uri,"expires_in":60,
                                "interval":if matches!(scenario, DeviceScenario::Wait) { 5 } else { 1 }})
                        }
                        "/copilot/token" => {
                            assert_eq!(form.len(), 3);
                            assert_eq!(form["grant_type"], "urn:ietf:params:oauth:grant-type:device_code");
                            assert!(form["device_code"] == COPILOT_DEVICE_CODE, "native device code retained privately");
                            match (scenario, index) {
                                (DeviceScenario::Progress, 1) => json!({"error":"authorization_pending"}),
                                (DeviceScenario::Progress, 2) => json!({"error":"slow_down"}),
                                (DeviceScenario::Progress, 3) => json!({"access_token":OAUTH_TOKEN,"expires_in":3600,"scope":"read:user"}),
                                (DeviceScenario::HeldReady, 1) => {
                                    timeout(BOUND, released.acquire()).await.unwrap().unwrap().forget();
                                    json!({"access_token":OAUTH_TOKEN,"expires_in":3600,"scope":"read:user"})
                                }
                                _ => panic!("unexpected native device poll or replay"),
                            }
                        }
                        _ => panic!("unexpected device endpoint, verification navigation or model request"),
                    };
                    assert_eq!(active.fetch_sub(1, Ordering::SeqCst), 1);
                    axum::Json(body)
                }
            },
        );
        let (stop, stopped) = oneshot::channel();
        let task = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _ = stopped.await;
                })
                .await
                .unwrap();
        });
        Self {
            origin,
            calls,
            release,
            stop: Some(stop),
            task: Some(task),
        }
    }

    async fn poll_received(&self) {
        timeout(BOUND, async {
            loop {
                if self
                    .calls
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|(path, _)| path == "/copilot/token")
                {
                    break;
                }
                sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("native device poll received");
    }

    async fn finish(&mut self) {
        let _ = self.stop.take().unwrap().send(());
        timeout(BOUND, self.task.take().unwrap())
            .await
            .unwrap()
            .unwrap();
    }
}

impl Drop for DeviceFixture {
    fn drop(&mut self) {
        if let Some(task) = &self.task {
            task.abort();
        }
    }
}

struct Cli {
    _root: Option<tempfile::TempDir>,
    prefix: PathBuf,
    state: PathBuf,
    master: OwnedFd,
    observer: OwnedFd,
    termios: Termios,
    flags: i32,
    child: Child,
    transcript: String,
    settled: bool,
    packaged: bool,
    observation_bound: Duration,
    retain_on_failure: bool,
    private_provider_setup: bool,
    initial_config: Vec<u8>,
    previous_preparations: BTreeSet<PathBuf>,
    setup_approvals: Vec<String>,
    denied_setup_review: Option<(&'static str, String)>,
}

impl Cli {
    fn spawn(origin: &str, provider: &str) -> Self {
        Self::spawn_selected(Some(origin), provider, None)
    }

    fn spawn_selected(origin: Option<&str>, provider: &str, fixture: Option<&Path>) -> Self {
        Self::spawn_product(origin, provider, fixture, false)
    }

    fn spawn_supplied(origin: Option<&str>, provider: &str, prefix: &Path) -> Self {
        Self::spawn_product(origin, provider, Some(prefix), true)
    }

    fn spawn_product(
        origin: Option<&str>,
        provider: &str,
        fixture: Option<&Path>,
        supplied: bool,
    ) -> Self {
        Self::spawn_configured(origin, provider, fixture, supplied, None, None)
    }

    fn spawn_oauth(origin: &str, oauth: &OAuthFixture) -> Self {
        Self::spawn_configured(Some(origin), &oauth.origin, None, false, Some(oauth), None)
    }

    fn spawn_device(origin: &str, device: &DeviceFixture) -> Self {
        Self::spawn_configured(
            Some(origin),
            &device.origin,
            None,
            false,
            None,
            Some(device),
        )
    }

    fn spawn_configured(
        origin: Option<&str>,
        provider: &str,
        fixture: Option<&Path>,
        supplied: bool,
        oauth: Option<&OAuthFixture>,
        device: Option<&DeviceFixture>,
    ) -> Self {
        let root = tempfile::tempdir().unwrap();
        Self::spawn_configured_in(root, origin, provider, fixture, supplied, oauth, device)
    }

    fn spawn_configured_in(
        root: tempfile::TempDir,
        origin: Option<&str>,
        provider: &str,
        fixture: Option<&Path>,
        supplied: bool,
        oauth: Option<&OAuthFixture>,
        device: Option<&DeviceFixture>,
    ) -> Self {
        let synthetic_prefix = fixture.is_none().then(|| {
            PathBuf::from(
                std::env::var_os("CONNECT_LAZY_SERVICES_FIXTURE_PREFIX").expect(
                    "PTY CLI tests require the assembled pin-built synthetic resource prefix",
                ),
            )
        });
        let fixture = fixture
            .or(synthetic_prefix.as_deref())
            .expect("selected resource prefix");
        let prefix = if supplied {
            fixture.to_owned()
        } else {
            root.path().join("product")
        };
        let bin = prefix.join("bin");
        let home = root.path().join("home");
        let state = root.path().join("state");
        let context = root.path().join("outside-checkout");
        let empty_path = root.path().join("empty-path");
        for directory in [&home, &context, &empty_path, &state.join(REALM)] {
            fs::create_dir_all(directory).unwrap();
        }
        if !supplied {
            fs::create_dir_all(&bin).unwrap();
        }
        let fixture_binary = fixture.join("bin/meerkat-connect");
        let source = fixture_binary.as_path();
        // A supplied production artifact may be built without test-support.
        // Its delivery owner supplies the exact receipt hash, as for stock rkat.
        // All ordinary fixtures keep the Cargo-built executable as their pin.
        let expected_cli_sha256 = supplied
            .then(|| std::env::var_os("CONNECT_SUPPLIED_CLI_SHA256"))
            .flatten()
            .map(|value| value.into_string().expect("CLI SHA-256 must be UTF-8"))
            .unwrap_or_else(|| file_digest(Path::new(env!("CARGO_BIN_EXE_meerkat-connect"))));
        assert_eq!(expected_cli_sha256.len(), 64);
        assert!(
            expected_cli_sha256
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        );
        assert_eq!(
            file_digest(source),
            expected_cli_sha256,
            "the selected fixture must contain the exact expected executable bytes"
        );
        let binary = bin.join("meerkat-connect");
        let reuse_prefix = binary.exists();
        if supplied || reuse_prefix {
            assert_eq!(
                file_digest(&binary),
                expected_cli_sha256,
                "the installed prefix must contain the exact expected executable bytes"
            );
        } else {
            fs::copy(source, &binary).unwrap();
            assert_eq!(
                Sha256::digest(fs::read(source).unwrap()),
                Sha256::digest(fs::read(&binary).unwrap())
            );
        }
        {
            let manifest =
                fs::read(fixture.join("libexec/meerkat-connect/resources.json")).unwrap();
            #[allow(
                clippy::option_env_unwrap,
                reason = "only this path needs the resource pin; a test build without it never reaches it"
            )]
            let pin = option_env!("MEERKAT_CONNECT_RESOURCES_SHA256")
                .expect("test and binary must be built with the resource pin");
            assert_eq!(format!("{:x}", Sha256::digest(&manifest)), pin);
            if !supplied && !reuse_prefix {
                let mut budget = (0usize, 0u64);
                copy_fixture_tree(
                    &fixture.join("libexec"),
                    &prefix.join("libexec"),
                    &mut budget,
                );
            }
            assert_eq!(
                fs::read(prefix.join("libexec/meerkat-connect/resources.json")).unwrap(),
                manifest
            );
        }
        // Public native config, not an injected LlmClient, RKAT_TEST_CLIENT,
        // provider override environment variable, or product test switch.
        let config = if device.is_some() {
            format!(
                r#"
[agent]
model = "{OAUTH_MODEL}"
max_turns = 4
[realm.cli-frontend]
default_binding = "copilot"
[realm.cli-frontend.backend.copilot]
provider = "anthropic"
backend_kind = "copilot"
[realm.cli-frontend.auth.copilot]
provider = "anthropic"
auth_method = "github_copilot_oauth"
source = {{kind = "managed_store"}}
[realm.cli-frontend.binding.copilot]
backend_profile = "copilot"
auth_profile = "copilot"
credential_account = "github_copilot"
default_model = "{OAUTH_MODEL}"
"#
            )
        } else if oauth.is_some() {
            format!(
                r#"
[agent]
model = "{OAUTH_MODEL}"
max_turns = 4
[realm.cli-frontend]
default_binding = "browser"
[realm.cli-frontend.backend.browser]
provider = "anthropic"
backend_kind = "anthropic_api"
base_url = "{provider}"
[realm.cli-frontend.auth.browser]
provider = "anthropic"
auth_method = "claude_ai_oauth"
source = {{kind = "managed_store"}}
[realm.cli-frontend.binding.browser]
backend_profile = "browser"
auth_profile = "browser"
default_model = "{OAUTH_MODEL}"
"#
            )
        } else {
            format!(
                r#"
[agent]
model = "cli-fixture"
max_turns = {max_turns}
[self_hosted.servers.cli]
transport = "openai_compatible"
api_style = "chat_completions"
base_url = "{provider}/v1"
[self_hosted.models.cli-fixture]
server = "cli"
remote_model = "cli-fixture-remote"
display_name = "CLI loopback fixture"
family = "cli-fixture"
context_window = {context_window}
max_output_tokens = 1024
call_timeout_secs = 10
[realm.cli-frontend]
default_binding = "fixture"
[realm.cli-frontend.backend.fixture]
provider = "self_hosted"
backend_kind = "self_hosted"
server = "cli"
base_url = "{provider}/v1"
[realm.cli-frontend.auth.fixture]
provider = "self_hosted"
auth_method = "static_bearer"
source = {{kind = "env", env = "CONNECT_FIXTURE_BEARER"}}
[realm.cli-frontend.binding.fixture]
backend_profile = "fixture"
auth_profile = "fixture"
default_model = "cli-fixture"
"#,
                max_turns = if supplied || origin.is_none() { 28 } else { 8 },
                // This finite composition case checks unabridged catalog/history.
                // It does not emulate a separate native compaction model request.
                context_window = if supplied { 131072 } else { 32768 },
            )
        };
        let config = if oauth.is_some_and(|fixture| fixture.managed_api_key) {
            config.replace(
                "auth_method = \"claude_ai_oauth\"",
                "auth_method = \"api_key\"",
            )
        } else {
            config
        };
        meerkat_core::Config::from_persisted_toml(&config)
            .unwrap()
            .0
            .validate(meerkat_models::canonical())
            .unwrap();
        let config_path = state.join(REALM).join("config.toml");
        if config_path.exists() {
            // A second ordinary process reuses the first process's saved setup
            // descriptor. Never erase it as test setup before the old/new review.
            let retained = fs::read(&config_path).unwrap();
            let mut actual =
                meerkat_core::Config::from_persisted_toml(std::str::from_utf8(&retained).unwrap())
                    .unwrap()
                    .0;
            actual
                .tools
                .mcp_servers
                .retain(|server| server.name != "toolkit-setup");
            let expected = meerkat_core::Config::from_persisted_toml(&config)
                .unwrap()
                .0;
            assert_eq!(
                serde_json::to_value(actual).unwrap(),
                serde_json::to_value(expected).unwrap()
            );
        } else {
            fs::write(&config_path, config).unwrap();
        }
        let initial_config = fs::read(&config_path).unwrap();
        let previous_preparations = preparation_artifacts(&state).into_iter().collect();
        let pty = openpty(None, None).unwrap();
        let observer = pty.slave.try_clone().unwrap();
        let termios = tcgetattr(&observer).unwrap();
        let flags_before_output = fcntl(&observer, FcntlArg::F_GETFL).unwrap();
        let original = OFlag::from_bits_retain(fcntl(&pty.master, FcntlArg::F_GETFL).unwrap());
        fcntl(&pty.master, FcntlArg::F_SETFL(original | OFlag::O_NONBLOCK)).unwrap();
        // The child shares this slave description for input and output. Darwin
        // adds its kernel-owned FWASWRITTEN bit on the first output write; that
        // bit is not settable through F_SETFL (sys/fcntl.h, FCNTLFLAGS). Establish
        // and check this cause before capturing the exact restoration baseline.
        assert_eq!(write(&pty.slave, b".").unwrap(), 1);
        let mut output_control = [0; 1];
        assert_eq!(read(&pty.master, &mut output_control).unwrap(), 1);
        assert_eq!(output_control, *b".");
        let flags = fcntl(&observer, FcntlArg::F_GETFL).unwrap();
        #[cfg(target_os = "macos")]
        assert_eq!(flags ^ flags_before_output, 0x0001_0000, "FWASWRITTEN");
        #[cfg(not(target_os = "macos"))]
        assert_eq!(flags, flags_before_output);
        let mut command = Command::new(binary);
        if oauth.is_some_and(|fixture| fixture.managed_api_key) {
            // Anthropic defers readiness to materialization. Ask for explicit
            // configured-target entry without overriding that native behavior.
            command.arg("--choose-model");
        }
        if let Some(origin) = origin {
            command.args(["--paired-host", origin, "--paired-host-memory-only"]);
        }
        command
            .args([
                "--model",
                if oauth.is_some() || device.is_some() {
                    OAUTH_MODEL
                } else {
                    "cli-fixture"
                },
                "--state-root",
            ])
            .arg(&state)
            .arg("--context-root")
            .arg(&context)
            .arg("--user-config-root")
            .arg(&home)
            .env_clear()
            .env("HOME", &home)
            .env("PATH", &empty_path)
            .env("TMPDIR", root.path())
            .current_dir(&context)
            .process_group(0)
            .stdin(Stdio::from(pty.slave.try_clone().unwrap()))
            .stdout(Stdio::from(pty.slave.try_clone().unwrap()))
            .stderr(Stdio::from(pty.slave));
        if let Some(origin) = oauth
            .map(|fixture| fixture.origin.as_str())
            .or_else(|| device.map(|fixture| fixture.origin.as_str()))
        {
            command
                .env("MEERKAT_TEST_OAUTH_ENDPOINT_OVERRIDE", "1")
                .env("MEERKAT_TEST_OAUTH_BASE_URL", origin)
                .env("XDG_CONFIG_HOME", home.join("native-config"));
        } else {
            command.env("CONNECT_FIXTURE_BEARER", TOKEN);
        }
        let child = command.spawn().unwrap();
        Self {
            _root: Some(root),
            prefix,
            state,
            master: pty.master,
            observer,
            termios,
            flags,
            child,
            transcript: String::new(),
            settled: false,
            packaged: true,
            observation_bound: if supplied {
                SUPPLIED_LIBRARY_BOUND
            } else {
                BOUND
            },
            retain_on_failure: supplied,
            private_provider_setup: oauth.is_some() || device.is_some(),
            initial_config,
            previous_preparations,
            setup_approvals: Vec::new(),
            denied_setup_review: None,
        }
    }

    fn diagnostics(&self) -> &str {
        if self.private_provider_setup {
            "private provider-setup transcript withheld"
        } else {
            &self.transcript
        }
    }

    fn drain(&mut self) {
        let mut buffer = [0; 4096];
        loop {
            match read(&self.master, &mut buffer) {
                Ok(0) | Err(Errno::EAGAIN) | Err(Errno::EIO) => break,
                Ok(count) => self
                    .transcript
                    .push_str(&String::from_utf8_lossy(&buffer[..count])),
                Err(error) => panic!("owned PTY read: {error}"),
            }
        }
        assert!(
            self.transcript.len() < 1024 * 1024,
            "bounded terminal output"
        );
        for secret in [
            CODE,
            DEVICE,
            PAIR,
            TOKEN,
            OAUTH_CODE,
            OAUTH_TOKEN,
            OAUTH_REFRESH,
            COPILOT_DEVICE_CODE,
        ] {
            assert!(
                !self.transcript.contains(secret),
                "protected fixture value reached the terminal"
            );
        }
    }

    async fn shown(&mut self, text: &str, from: usize) {
        let deadline = Instant::now() + self.observation_bound;
        loop {
            self.drain();
            if self.transcript[from..].contains(text) {
                return;
            }
            assert!(
                self.child.try_wait().unwrap().is_none(),
                "CLI exited before {text}: {}",
                self.diagnostics()
            );
            assert!(
                Instant::now() < deadline,
                "CLI did not show {text}: {}",
                self.diagnostics()
            );
            sleep(Duration::from_millis(5)).await;
        }
    }

    fn send(&self, line: &str) {
        assert_eq!(write(&self.master, line.as_bytes()).unwrap(), line.len());
    }

    async fn approve_setup_helper(&mut self) {
        assert!(self.setup_approvals.is_empty());
        self.shown(SETUP_HELPER_REVIEW, 0).await;
        assert_eq!(
            fs::read(self.state.join(REALM).join("config.toml")).unwrap(),
            self.initial_config,
            "the native config mutation must wait for this exact review"
        );
        self.approve_startup_review(SETUP_HELPER_REVIEW).await;
    }

    async fn enter_conversation(&mut self) {
        assert_eq!(self.setup_approvals.len(), 1);
        self.approve_startup_review(SETUP_MODEL_REVIEW).await;
        self.shown("You: ", 0).await;
        assert_eq!(self.setup_approvals.len(), 2);
    }

    async fn approve_startup_review(&mut self, summary: &str) {
        assert!(self.denied_setup_review.is_none());
        self.shown(summary, 0).await;
        let start = self.transcript.rfind(summary).unwrap();
        self.shown("Type 'approve ", start).await;
        let id = self.transcript[start..]
            .split("Type 'approve ")
            .nth(1)
            .unwrap()
            .split('\'')
            .next()
            .unwrap()
            .to_owned();
        assert!(!id.is_empty() && !self.setup_approvals.contains(&id));
        assert!(!self.transcript.contains("Setup session: "));
        self.send(&format!("approve {id}\n"));
        self.setup_approvals.push(id);
    }

    async fn deny_startup_review(&mut self, summary: &'static str) {
        assert!(self.denied_setup_review.is_none());
        assert_eq!(
            self.setup_approvals.len(),
            match summary {
                SETUP_HELPER_REVIEW => 0,
                SETUP_MODEL_REVIEW => 1,
                _ => panic!("only the two exact startup reviews belong to this fixture"),
            }
        );
        self.shown(summary, 0).await;
        let start = self.transcript.rfind(summary).unwrap();
        self.shown("Type 'approve ", start).await;
        let id = self.transcript[start..]
            .split("Type 'approve ")
            .nth(1)
            .unwrap()
            .split('\'')
            .next()
            .unwrap()
            .to_owned();
        assert!(!id.is_empty() && !self.setup_approvals.contains(&id));
        assert!(!self.transcript.contains("Setup session: "));
        self.send(&format!("deny {id}\n"));
        self.denied_setup_review = Some((summary, id));
    }

    fn assert_reviewed_setup(&self, before: &[u8]) {
        self.assert_reviewed_setup_with_installations(before, &[]);
    }

    fn assert_reviewed_setup_with_installations(
        &self,
        before: &[u8],
        installed: &[meerkat_core::McpServerConfig],
    ) {
        assert_eq!(before, self.initial_config);
        let mut expected =
            meerkat_core::Config::from_persisted_toml(std::str::from_utf8(before).unwrap())
                .unwrap()
                .0;
        let current = fs::read(self.state.join(REALM).join("config.toml")).unwrap();
        let actual =
            meerkat_core::Config::from_persisted_toml(std::str::from_utf8(&current).unwrap())
                .unwrap()
                .0;
        let new: Vec<_> = preparation_artifacts(&self.state)
            .into_iter()
            .filter(|path| !self.previous_preparations.contains(path))
            .collect();
        assert_eq!(
            new.len(),
            1,
            "exactly one supplied preparation per CLI process"
        );
        let (_, prepared) = read_bounded_json(&new[0].join("prepare.stdout"), 256 * 1024);
        let stdio: meerkat_core::mcp_config::McpStdioConfig =
            serde_json::from_value(prepared["mcpServers"]["toolkit-setup"].clone()).unwrap();
        let mut server = meerkat_core::McpServerConfig::stdio(
            "toolkit-setup",
            stdio.command,
            stdio.args,
            stdio.env,
        );
        let operations = [
            "setup_state",
            "setup_jobs",
            "setup_job",
            "setup_catalog",
            "setup_catalog_entry",
            "setup_catalog_route",
            "setup_prepare",
            "setup_retry_prepare",
            "setup_help",
            "setup_check",
            "setup_stop",
            "setup_reconcile",
            "setup_recovery_manifest",
            "setup_guide",
            "setup_inspect_package",
            "setup_inspect_schema",
            "setup_acquire_python",
            "setup_search_node",
            "setup_acquire_node",
            "setup_acquire_node_prebuilt",
            "setup_inspect_acquired",
            "setup_recipes",
            "setup_recipe",
            "setup_remember_remote",
            "setup_export_recipe",
        ];
        server.access = Some(meerkat_core::McpServerAccess::allowing(operations));
        let prior: Vec<_> = expected
            .tools
            .mcp_servers
            .iter()
            .enumerate()
            .filter(|(_, row)| row.name == "toolkit-setup")
            .collect();
        assert!(prior.len() <= 1);
        let previous = prior.first().map(|(_, row)| (*row).clone());
        let index = prior.first().map(|(index, _)| *index);
        let (_, review) =
            read_bounded_json(&new[0].join("standalone-setup-review.txt"), 256 * 1024);
        assert_eq!(review["realm"], REALM);
        assert_eq!(
            review["previous_setup"],
            serde_json::to_value(previous).unwrap()
        );
        assert_eq!(
            review["supplied_setup"],
            serde_json::to_value(&server).unwrap()
        );
        let checked = &review["checked_installation"];
        assert_eq!(checked["route"], "meerkat-connect-checked-installs");
        assert_eq!(checked["server"], "toolkit-setup");
        assert_eq!(
            checked["operations"],
            json!([
                "setup_install",
                "setup_selected_check",
                "setup_selected_install",
            ]),
            "the initial review must name the exact separately granted route operations"
        );
        let scope = checked["scope"].as_str().unwrap();
        assert!(scope.contains("live checked setup session"));
        assert!(scope.contains("separate human review"));
        assert!(scope.contains("raw MCP access does not include these operations"));
        assert!(
            review["lifetime"]
                .as_str()
                .unwrap()
                .contains("Until revoked")
        );
        assert!(
            self.transcript
                .contains("Checked installation route access was granted and selected")
        );
        match index {
            Some(index) => expected.tools.mcp_servers[index] = server,
            None => expected.tools.mcp_servers.push(server),
        }
        for server in installed {
            assert!(
                !expected
                    .tools
                    .mcp_servers
                    .iter()
                    .any(|row| row.name == server.name),
                "an additional reviewed installation must have its own server name"
            );
            expected.tools.mcp_servers.push(server.clone());
        }
        assert_eq!(
            serde_json::to_value(actual).unwrap(),
            serde_json::to_value(expected).unwrap(),
            "only exact reviewed supplied MCP descriptors may change"
        );
        assert!(self.state.join(REALM).join("realm_manifest.json").is_file());
        assert!(
            self.transcript
                .contains("Setup helper access was granted and selected")
        );
    }

    fn reviewed_session(&self) -> meerkat_core::SessionId {
        let new: Vec<_> = preparation_artifacts(&self.state)
            .into_iter()
            .filter(|path| !self.previous_preparations.contains(path))
            .collect();
        assert_eq!(new.len(), 1);
        reviewed_setup_session(&new[0], &self.setup_approvals, &self.denied_setup_review)
    }

    async fn finish(&mut self) -> ExitStatus {
        self.finish_with_installations(&[]).await
    }

    async fn finish_with_installations(
        &mut self,
        installed: &[meerkat_core::McpServerConfig],
    ) -> ExitStatus {
        let deadline = Instant::now() + self.observation_bound;
        let status = loop {
            self.drain();
            if let Some(status) = self.child.try_wait().unwrap() {
                break status;
            }
            assert!(
                Instant::now() < deadline,
                "CLI did not exit: {}",
                self.diagnostics()
            );
            sleep(Duration::from_millis(5)).await;
        };
        self.drain();
        assert_eq!(tcgetattr(&self.observer).unwrap(), self.termios);
        assert_eq!(
            fcntl(&self.observer, FcntlArg::F_GETFL).unwrap(),
            self.flags
        );
        assert_eq!(self.prefix.join("libexec").exists(), self.packaged);
        if self
            .denied_setup_review
            .as_ref()
            .is_some_and(|(summary, _)| *summary == SETUP_HELPER_REVIEW)
        {
            assert!(self.setup_approvals.is_empty());
            assert_eq!(
                self.transcript
                    .matches("Preparing the supplied setup helper.")
                    .count(),
                1
            );
            let new: Vec<_> = preparation_artifacts(&self.state)
                .into_iter()
                .filter(|path| !self.previous_preparations.contains(path))
                .collect();
            assert_eq!(
                new.len(),
                1,
                "the prepared resource and denied decision remain inspectable"
            );
            assert_eq!(
                fs::read(self.state.join(REALM).join("config.toml")).unwrap(),
                self.initial_config
            );
            assert_eq!(
                meerkat_store::read_realm_posture_in(&self.state, REALM)
                    .await
                    .unwrap(),
                meerkat_store::RealmPosture::NotMaterialized
            );
            assert!(!meerkat_store::grant_owner_store_path(&self.state.join(REALM)).exists());
            assert!(!self.state.join(REALM).join("config_state.json").exists());
            assert!(!new[0].join("standalone-setup-review.txt").exists());
            self.reviewed_session();
            assert!(!self.transcript.contains("Setup helper access was granted"));
            assert!(!self.transcript.contains(SETUP_MODEL_REVIEW));
            assert!(!self.transcript.contains("Setup session: "));
            assert!(!self.transcript.contains("You: "));
        } else if self.setup_approvals.is_empty() {
            assert!(
                !self
                    .transcript
                    .contains("Preparing the supplied setup helper.")
            );
            assert_eq!(
                preparation_artifacts(&self.state)
                    .into_iter()
                    .collect::<BTreeSet<_>>(),
                self.previous_preparations
            );
        } else {
            self.assert_reviewed_setup_with_installations(&self.initial_config, installed);
            assert_eq!(
                self.transcript
                    .matches("Preparing the supplied setup helper.")
                    .count(),
                1
            );
            assert_eq!(
                meerkat_store::read_realm_posture_in(&self.state, REALM)
                    .await
                    .unwrap(),
                meerkat_store::RealmPosture::Governed,
                "the reviewed pre-stage governance remains after CLI exit"
            );
            let selected = self.reviewed_session();
            if let Some(banner) = self.transcript.split("Setup session: ").nth(1) {
                assert_eq!(
                    banner.split_whitespace().next().unwrap(),
                    selected.to_string()
                );
            }
        }
        assert_eq!(
            killpg(Pid::from_raw(self.child.id() as i32), None),
            Err(Errno::ESRCH),
            "the exact test-created process group must be absent"
        );
        self.settled = true;
        status
    }
}

fn copy_fixture_tree(source: &Path, destination: &Path, budget: &mut (usize, u64)) {
    let metadata = fs::symlink_metadata(source).unwrap();
    budget.0 += 1;
    assert!(budget.0 <= 64 && !metadata.file_type().is_symlink());
    if metadata.is_dir() {
        fs::create_dir_all(destination).unwrap();
        for entry in fs::read_dir(source).unwrap() {
            let entry = entry.unwrap();
            copy_fixture_tree(&entry.path(), &destination.join(entry.file_name()), budget);
        }
    } else {
        assert!(metadata.is_file());
        budget.1 += metadata.len();
        assert!(
            budget.1 <= 1024 * 1024,
            "synthetic resources must stay bounded"
        );
        fs::copy(source, destination).unwrap();
    }
    fs::set_permissions(destination, metadata.permissions()).unwrap();
}

impl Drop for Cli {
    fn drop(&mut self) {
        if !self.settled {
            // Exact test-created process group only. This is operator cleanup
            // after a failed assertion, never evidence of native cancellation.
            let _ = killpg(Pid::from_raw(self.child.id() as i32), Signal::SIGKILL);
            let _ = self.child.wait();
        }
        if self.retain_on_failure
            && std::thread::panicking()
            && let Some(root) = self._root.take()
        {
            eprintln!(
                "Real supplied CLI failure artifacts retained at {}",
                root.keep().display()
            );
        }
    }
}

#[tokio::test]
async fn actual_cli_pairs_without_os_store_and_submits_exact_review_after_supplied_setup() {
    let mut host = Server::start(false).await;
    let mut provider = Server::start(true).await;
    let mut cli = Cli::spawn(&host.origin, &provider.origin);
    cli.shown("New realm: ", 0).await;
    cli.send("not a valid realm\n");
    cli.shown("That name is not valid", 0).await;
    cli.send(&format!("{REALM}\n"));
    cli.approve_setup_helper().await;
    cli.shown("Type 'request'", 0).await;
    assert!(
        cli.transcript
            .contains("The OS credential store will not be read or written")
    );
    assert!(host.requests.lock().unwrap().is_empty());
    cli.send("request\n");
    cli.shown("Enter the six-digit code (hidden)", 0).await;
    timeout(BOUND, async {
        while tcgetattr(&cli.observer)
            .unwrap()
            .local_flags
            .contains(LocalFlags::ECHO)
        {
            sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .unwrap();
    cli.send(&format!("{CODE}\n"));
    cli.enter_conversation().await;
    assert_eq!(tcgetattr(&cli.observer).unwrap(), cli.termios);
    assert!(
        provider.requests.lock().unwrap().is_empty(),
        "no model request before human input"
    );
    let turn_start = cli.transcript.len();
    cli.send("Read the frontend inventory and prepare, submit and check its review request.\n");
    cli.shown("Type 'approve ", turn_start).await;
    let approval = cli.transcript[turn_start..]
        .split("Type 'approve ")
        .last()
        .unwrap()
        .split('\'')
        .next()
        .unwrap()
        .to_owned();
    assert!(!approval.is_empty());
    assert!(cli.transcript.contains(&host.origin));
    assert!(
        cli.transcript
            .contains(prepared()["change"]["change_digest"].as_str().unwrap())
    );
    assert!(
        cli.transcript
            .replace("\r\n", "\n")
            .contains(prepared()["change"]["unified_diff"].as_str().unwrap())
    );
    assert_eq!(host.submissions(), 0);
    cli.send("approve old-approval-id\n");
    cli.shown("Use the exact approval ID shown above", 0).await;
    assert_eq!(host.submissions(), 0);
    let after_decision = cli.transcript.len();
    cli.send(&format!("approve {approval}\n"));
    cli.shown(ANSWER, after_decision).await;
    cli.shown("You: ", after_decision).await;
    assert_eq!(host.submissions(), 1);
    assert_eq!(provider.requests.lock().unwrap().len(), 5);
    assert!(!cli.transcript.contains("submission response is uncertain"));
    cli.send("/quit\n");
    assert!(cli.finish().await.success(), "{}", cli.transcript);
    host.finish().await;
    provider.finish().await;
    let requests = host.requests.lock().unwrap();
    assert_eq!(
        requests.len(),
        6,
        "two pairing calls and four native frontend calls"
    );
    assert_eq!(
        requests[4].body,
        json!({"change_digest":prepared()["change"]["change_digest"],"change":prepared()["change"]})
    );
    assert_eq!(requests[5].method, "GET");
    assert!(
        requests[5]
            .path
            .ends_with(prepared()["change"]["change_digest"].as_str().unwrap())
    );
    assert!(cli.state.join(REALM).join("realm_manifest.json").is_file());
}

#[tokio::test]
async fn actual_cli_failed_uncertain_submission_allows_only_explicit_exact_status_turn() {
    use meerkat_runtime::{
        completion::CompletionOutcome,
        identifiers::LogicalRuntimeId,
        input_state::{InputLifecycleState, InputTerminalOutcome},
    };

    let mut host = Server::start_recovery(false).await;
    let mut provider = Server::start_recovery(true).await;
    let mut cli = Cli::spawn(&host.origin, &provider.origin);
    cli.shown("New realm: ", 0).await;
    cli.send(&format!("{REALM}\n"));
    cli.approve_setup_helper().await;
    pair_fixture_cli(&mut cli).await;
    cli.enter_conversation().await;
    assert!(provider.requests.lock().unwrap().is_empty());
    let first_turn = cli.transcript.len();
    cli.send(&format!("{RECOVERY_REQUEST}\n"));
    cli.shown("Type 'approve ", first_turn).await;
    let approval = cli.transcript[first_turn..]
        .split("Type 'approve ")
        .last()
        .unwrap()
        .split('\'')
        .next()
        .unwrap()
        .to_owned();
    assert!(!approval.is_empty());
    assert_eq!(host.submissions(), 0);
    let approved = cli.transcript.len();
    cli.send(&format!("approve {approval}\n"));
    cli.shown(
        "The native setup turn failed. No automatic retry was started.",
        approved,
    )
    .await;
    cli.shown(
        "Only status checks for that exact request remain available",
        approved,
    )
    .await;
    cli.shown("You: ", approved).await;
    assert_eq!(provider.requests.lock().unwrap().len(), 4);
    assert_eq!(host.requests.lock().unwrap().len(), 5);
    assert_eq!(host.submissions(), 1);
    assert!(!cli.transcript[approved..].contains(ANSWER));
    assert!(!cli.transcript[approved..].contains(STATUS_ANSWER));
    let second_turn = cli.transcript.len();
    cli.send(&format!("{STATUS_REQUEST}\n"));
    cli.shown(STATUS_ANSWER, second_turn).await;
    cli.shown("You: ", second_turn).await;
    assert!(!cli.transcript[second_turn..].contains("Type 'approve "));
    assert_eq!(host.submissions(), 1);
    assert_eq!(host.requests.lock().unwrap().len(), 6);
    assert_eq!(provider.requests.lock().unwrap().len(), 12);
    cli.send("/quit\n");
    assert!(
        !cli.finish().await.success(),
        "uncertain custody survives readback"
    );
    assert!(cli.transcript.contains("Its approval is spent"));
    assert_eq!(cli.transcript.matches("Type 'approve ").count(), 3);
    assert_eq!(cli.transcript.matches("Setup session: ").count(), 1);
    assert_eq!(
        cli.transcript
            .matches("Enter the six-digit code (hidden)")
            .count(),
        1
    );
    assert_eq!(preparation_output_count(&cli.state), 1);
    host.finish().await;
    provider.finish().await;
    {
        let requests = host.requests.lock().unwrap();
        let digest = prepared()["change"]["change_digest"]
            .as_str()
            .unwrap()
            .to_owned();
        assert_eq!(requests[4].method, "POST");
        assert_eq!(
            requests[4].body,
            json!({"change_digest":digest,"change":prepared()["change"]})
        );
        assert_eq!(requests[5].method, "GET");
        assert_eq!(
            requests[5].path,
            format!("/console/extensions/frontend/change-requests/{digest}")
        );
        assert!(requests[5].body.is_null());
        for request in requests
            .iter()
            .filter(|request| request.path.starts_with("/console/extensions/"))
        {
            assert_eq!(
                request.headers["cookie"],
                format!("paired_host_device={DEVICE}")
            );
            if request.method == "POST" {
                assert_eq!(request.headers["origin"], host.origin);
            }
            assert!(!request.headers.contains_key("x-host-dev-token"));
        }
    }
    for request in provider.requests.lock().unwrap().iter() {
        assert!(!request.body.to_string().contains(&host.origin));
    }

    // Reopen only the test's existing native persistence after the executable
    // has joined. No agent/runtime is created or resumed for this readback.
    let session_id = meerkat_core::SessionId::parse(
        cli.transcript
            .split("Setup session: ")
            .nth(1)
            .unwrap()
            .split_whitespace()
            .next()
            .unwrap(),
    )
    .unwrap();
    let root = cli._root.as_ref().unwrap().path();
    let storage = meerkat_connect::RealmOptions {
        context_root: root.join("outside-checkout"),
        state_root: Some(cli.state.clone()),
        user_config_root: Some(root.join("home")),
    }
    .resolve(REALM)
    .unwrap();
    let (_, persistence) = timeout(
        BOUND,
        meerkat::governed_realm::open_stock_realm_persistence_as(
            &meerkat::DiskStorageProvider,
            &storage.locator.state_root,
            REALM,
            None,
            Some(meerkat_store::RealmOrigin::Explicit),
            Some(storage.layout),
            meerkat::governed_realm::stock::issuance::DEFAULT_STOCK_AGENT,
        ),
    )
    .await
    .unwrap()
    .unwrap();
    let mut inputs = timeout(
        BOUND,
        persistence
            .runtime_store()
            .load_input_states_strict(&LogicalRuntimeId::for_session(&session_id)),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(
        inputs.len(),
        2,
        "one failed input and one explicit new status input"
    );
    inputs.sort_by_key(|input| input.seed.admission_sequence);
    for input in &inputs {
        assert_eq!(input.seed.phase, InputLifecycleState::Consumed);
        assert_eq!(
            input.seed.terminal_outcome,
            Some(InputTerminalOutcome::Consumed)
        );
        assert_eq!(input.seed.attempt_count, 1, "no runtime replay");
    }
    let first = serde_json::to_value(&inputs[0]).unwrap();
    let outcome: CompletionOutcome =
        serde_json::from_value(first["terminal_completion"]["outcome"].clone()).unwrap();
    assert!(matches!(
        outcome,
        CompletionOutcome::Abandoned { .. } | CompletionOutcome::AbandonedWithError { .. }
    ));
    assert!(
        first["terminal_completion"]["outcome"]
            .to_string()
            .contains("fixture_unavailable_tool")
    );
    let second = serde_json::to_value(&inputs[1]).unwrap();
    let outcome: CompletionOutcome =
        serde_json::from_value(second["terminal_completion"]["outcome"].clone()).unwrap();
    assert!(matches!(outcome, CompletionOutcome::Completed(_)));

    let mut paths = vec![cli.state.clone()];
    let mut count = (0usize, 0u64);
    let mut decisions = 0;
    let mut stored_messages = (false, false);
    while let Some(directory) = paths.pop() {
        for entry in fs::read_dir(directory).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path).unwrap();
            assert!(!metadata.file_type().is_symlink());
            if metadata.is_dir() {
                paths.push(path);
                continue;
            }
            assert!(metadata.is_file());
            count.0 += 1;
            count.1 += metadata.len();
            assert!(count.0 <= 256 && count.1 <= 32 * 1024 * 1024);
            let bytes = fs::read(&path).unwrap();
            for protected in [CODE, DEVICE, PAIR, TOKEN] {
                assert!(
                    !bytes
                        .windows(protected.len())
                        .any(|part| part == protected.as_bytes())
                );
            }
            let visible = String::from_utf8_lossy(&bytes);
            stored_messages.0 |= visible.contains(RECOVERY_REQUEST);
            stored_messages.1 |= visible.contains(STATUS_REQUEST);
            if path.file_name().and_then(|name| name.to_str()) == Some("native-decisions.json") {
                decisions += 1;
                assert!(visible.contains(&approval));
            }
        }
    }
    assert_eq!(decisions, 1);
    assert_eq!(stored_messages, (true, true));
}

#[tokio::test]
async fn actual_cli_secret_intent_approval_hands_entry_only_to_terminal_and_reads_host_status() {
    let mut host = Server::start_scenario(false, true).await;
    let mut provider = Server::start_scenario(true, true).await;
    let mut cli = Cli::spawn(&host.origin, &provider.origin);
    let review_path = secret_prepared()["review_url"].as_str().unwrap().to_owned();
    let review_url = format!("{}{review_path}", host.origin);
    cli.shown("New realm: ", 0).await;
    cli.send(&format!("{REALM}\n"));
    cli.approve_setup_helper().await;
    cli.shown("Type 'request'", 0).await;
    assert!(host.requests.lock().unwrap().is_empty());
    cli.send("request\n");
    cli.shown("Enter the six-digit code (hidden)", 0).await;
    timeout(BOUND, async {
        while tcgetattr(&cli.observer)
            .unwrap()
            .local_flags
            .contains(LocalFlags::ECHO)
        {
            sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .unwrap();
    cli.send(&format!("{CODE}\n"));
    cli.enter_conversation().await;
    assert!(provider.requests.lock().unwrap().is_empty());
    let turn_start = cli.transcript.len();
    cli.send("Read and prepare the selected frontend. Request its missing secret binding, then report the host's status without entering or confirming a value.\n");
    cli.shown("Type 'approve ", turn_start).await;
    let approval = cli.transcript[turn_start..]
        .split("Type 'approve ")
        .last()
        .unwrap()
        .split('\'')
        .next()
        .unwrap()
        .to_owned();
    assert!(!approval.is_empty());
    let review = &cli.transcript[turn_start..];
    let retained_request = secret_request();
    for retained in [
        host.origin.as_str(),
        "secret_intent_create",
        "telegram_bot_token",
        retained_request["base"]["ledger_digest"].as_str().unwrap(),
    ] {
        assert!(
            review.contains(retained),
            "exact binding must be reviewable before the effect"
        );
    }
    assert!(!review.contains(ENTRY_PATH));
    assert!(
        !review.contains(&review_path),
        "review output waits for native completion"
    );
    assert!(
        !host
            .requests
            .lock()
            .unwrap()
            .iter()
            .any(|request| request.path == INTENTS)
    );
    assert_eq!(provider.requests.lock().unwrap().len(), 3);
    cli.send("approve old-secret-approval-id\n");
    cli.shown("Use the exact approval ID shown above", turn_start)
        .await;
    assert!(
        !host
            .requests
            .lock()
            .unwrap()
            .iter()
            .any(|request| request.path == INTENTS)
    );
    let after_approval = cli.transcript.len();
    cli.send(&format!("approve {approval}\n"));
    cli.shown(SECRET_ANSWER, after_approval).await;
    cli.shown("You: ", after_approval).await;
    let result = &cli.transcript[after_approval..];
    let entry_url = format!("{}{ENTRY_PATH}", host.origin);
    assert_eq!(result.matches("Paired host secret entry").count(), 1);
    assert_eq!(result.matches("Paired host change review").count(), 1);
    assert_eq!(result.matches(&review_url).count(), 1);
    assert!(result.contains("Kind: channel_config"));
    assert!(
        result.contains(
            secret_prepared()["change"]["change_digest"]
                .as_str()
                .unwrap()
        )
    );
    assert!(result.find(&review_url).unwrap() < result.rfind("You: ").unwrap());
    assert_eq!(
        result.matches(&entry_url).count(),
        1,
        "default status reads must not redisplay the entry link"
    );
    assert!(result.contains("Pair that browser with the paired host independently."));
    assert!(result.contains("Never enter the secret in this conversation."));
    assert!(result.contains("Entry and owner confirmation happen in the paired host."));
    assert!(
        result.contains(
            "This link does not prove the secret was committed or the selected extension roles are ready."
        )
    );
    assert!(!result.contains(&format!("fixture-owner-label.invalid{ENTRY_PATH}")));
    assert!(
        result.find(&entry_url).unwrap() < result.rfind("You: ").unwrap(),
        "the queued handoff must be drained before returning to input"
    );
    assert!(!result.contains("uncertain"));
    assert_eq!(provider.requests.lock().unwrap().len(), 6);
    let repeat_start = cli.transcript.len();
    cli.send(&format!("{REVIEW_REQUEST}\n"));
    cli.shown(REVIEW_ANSWER, repeat_start).await;
    cli.shown("You: ", repeat_start).await;
    let repeated = &cli.transcript[repeat_start..];
    assert_eq!(repeated.matches("Paired host change review").count(), 1);
    assert_eq!(repeated.matches(&review_url).count(), 1);
    assert!(!repeated.contains("Type 'approve "));
    assert!(!repeated.contains(&entry_url));
    assert!(
        repeated.find(&review_url).unwrap() < repeated.find("You: ").unwrap(),
        "the queued review handoff must be drained before returning to input"
    );
    assert_eq!(cli.transcript.matches("Setup session: ").count(), 1);
    assert_eq!(provider.requests.lock().unwrap().len(), 8);
    cli.send("/quit\n");
    assert!(cli.finish().await.success(), "{}", cli.transcript);
    host.finish().await;
    provider.finish().await;
    for request in provider.requests.lock().unwrap().iter() {
        let body = request.body.to_string();
        assert!(!body.contains(&host.origin));
        assert!(!body.contains(&review_path));
    }

    let requests = host.requests.lock().unwrap();
    let status_path = format!("{INTENTS}/{INTENT_ID}");
    let draft_status_path = format!(
        "/console/extensions/frontend/change-requests/{}",
        secret_prepared()["change"]["change_digest"]
            .as_str()
            .unwrap()
    );
    assert_eq!(
        requests
            .iter()
            .map(|request| (request.method.as_str(), request.path.as_str()))
            .collect::<Vec<_>>(),
        vec![
            ("POST", "/console/pair/request"),
            ("POST", "/console/pair/confirm"),
            ("GET", "/console/extensions/frontend/inventory"),
            ("POST", "/console/extensions/frontend/prepare"),
            ("POST", INTENTS),
            ("GET", status_path.as_str()),
            ("GET", status_path.as_str()),
            ("GET", draft_status_path.as_str()),
        ],
        "one preparation, one secret creation and explicit status reads, with no repeat POST, submit, entry, confirm, cancel or harness route"
    );
    assert_eq!(requests[4].body, secret_request());
    drop(requests);

    // Read only this test's owned state after the child has joined. Inspect the
    // actual native session files and native approval records, including any
    // SQLite sidecars; the PTY transcript is intentionally kept only in memory.
    let mut pending = vec![cli.state.clone()];
    let mut file_count = 0;
    let mut total_bytes = 0;
    let mut approval_records = 0;
    let mut native_receipt_observed = false;
    let mut review_request_persisted = false;
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(directory).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            let kind = entry.file_type().unwrap();
            assert!(
                !kind.is_symlink(),
                "owned state must not lead outside the fixture"
            );
            if kind.is_dir() {
                pending.push(path);
                continue;
            }
            assert!(kind.is_file());
            file_count += 1;
            total_bytes += entry.metadata().unwrap().len();
            assert!(file_count <= 256 && total_bytes <= 32 * 1024 * 1024);
            let bytes = fs::read(&path).unwrap();
            for protected in [CODE, DEVICE, PAIR, TOKEN, ENTRY_PATH, review_path.as_str()] {
                assert!(
                    !bytes
                        .windows(protected.len())
                        .any(|part| part == protected.as_bytes()),
                    "private entry material persisted in {}",
                    path.display()
                );
            }
            review_request_persisted |= bytes
                .windows(REVIEW_REQUEST.len())
                .any(|part| part == REVIEW_REQUEST.as_bytes());
            if path.file_name().and_then(|name| name.to_str()) == Some("native-decisions.json") {
                approval_records += 1;
                let record: Value = serde_json::from_slice(&bytes).unwrap();
                let record = record.to_string();
                assert!(record.contains(&approval));
                assert!(record.contains("secret_intent_create"));
            } else if path.starts_with(cli.state.join(REALM)) {
                native_receipt_observed |= bytes
                    .windows(INTENT_ID.len())
                    .any(|part| part == INTENT_ID.as_bytes());
            }
        }
    }
    assert_eq!(approval_records, 1);
    assert!(
        review_request_persisted,
        "the explicit redisplay request remains in native history"
    );
    assert!(
        native_receipt_observed,
        "the scan must include persisted native intent observations"
    );
}

#[tokio::test]
async fn actual_cli_explicit_memory_pairing_can_exit_after_retained_setup_before_request() {
    let mut host = Server::start(false).await;
    let mut provider = Server::start(true).await;
    let mut cli = Cli::spawn(&host.origin, &provider.origin);
    cli.shown("New realm: ", 0).await;
    cli.send(&format!("{REALM}\n"));
    cli.approve_setup_helper().await;
    cli.shown("Type 'request'", 0).await;
    cli.send("/quit\n");
    assert!(cli.finish().await.success(), "{}", cli.transcript);
    host.finish().await;
    provider.finish().await;
    assert!(host.requests.lock().unwrap().is_empty());
    assert!(provider.requests.lock().unwrap().is_empty());
    assert!(cli.state.join(REALM).join("realm_manifest.json").is_file());
    cli.assert_reviewed_setup(&cli.initial_config);
}

async fn begin_provider_login(
    cli: &mut Cli,
    oauth: &OAuthFixture,
    host: &Server,
) -> (reqwest::Url, String) {
    cli.shown("New realm: ", 0).await;
    cli.send(&format!("{REALM}\n"));
    cli.approve_setup_helper().await;
    cli.shown("Model target (number, or /quit): ", 0).await;
    assert!(
        cli.transcript
            .contains("Owner realm: cli-frontend; binding: browser")
    );
    assert!(!cli.transcript.contains("Type 'request'"));
    assert!(!cli.transcript.contains("Setup session: "));
    assert!(host.requests.lock().unwrap().is_empty());
    assert!(oauth.requests.lock().unwrap().is_empty());
    cli.send("1\n");
    cli.shown("Type 'signin' to authenticate this exact owner", 0)
        .await;
    cli.send("signin\n");
    cli.shown("Waiting for the native browser callback", 0)
        .await;
    let authorize = reqwest::Url::parse(
        cli.transcript
            .lines()
            .find(|line| line.starts_with(&format!("{}/anthropic/authorize?", oauth.origin)))
            .expect("private synthetic authorization URL retained only in this test's memory"),
    )
    .unwrap();
    let query: BTreeMap<String, String> = authorize.query_pairs().into_owned().collect();
    assert_eq!(query["code_challenge_method"], "S256");
    assert!(!query["code_challenge"].is_empty());
    let redirect = reqwest::Url::parse(&query["redirect_uri"]).unwrap();
    assert_eq!(redirect.host_str(), Some("localhost"));
    assert_eq!(redirect.path(), "/callback");
    assert!(redirect.port().is_some());
    assert!(host.requests.lock().unwrap().is_empty());
    assert!(oauth.requests.lock().unwrap().is_empty());
    (redirect, query["state"].clone())
}

async fn deliver_provider_callback(redirect: &reqwest::Url, state: &str, denied: bool) {
    let mut callback = redirect.clone();
    callback
        .query_pairs_mut()
        .append_pair("state", state)
        .append_pair(
            if denied { "error" } else { "code" },
            if denied { "access_denied" } else { OAUTH_CODE },
        );
    let response = timeout(BOUND, reqwest::Client::new().get(callback).send())
        .await
        .unwrap()
        .unwrap_or_else(|_| panic!("private synthetic callback request failed"));
    assert_eq!(response.status().as_u16(), if denied { 400 } else { 200 });
    timeout(BOUND, response.bytes())
        .await
        .unwrap()
        .unwrap_or_else(|_| panic!("private synthetic callback response failed"));
}

async fn assert_provider_callback_retired(redirect: &reqwest::Url) {
    let result = timeout(
        BOUND,
        TcpStream::connect(("127.0.0.1", redirect.port().unwrap())),
    )
    .await
    .unwrap();
    assert!(
        result.is_err(),
        "native callback listener must be joined before CLI completion"
    );
}

#[tokio::test]
async fn actual_cli_configured_browser_login_precedes_pairing_and_uses_same_native_session() {
    let mut host = Server::start(false).await;
    let mut oauth = OAuthFixture::start(false, false).await;
    let mut cli = Cli::spawn_oauth(&host.origin, &oauth);
    let config = fs::read(cli.state.join(REALM).join("config.toml")).unwrap();
    let (redirect, state) = begin_provider_login(&mut cli, &oauth, &host).await;
    deliver_provider_callback(&redirect, &state, false).await;
    cli.shown("Native sign-in completed for the selected owner", 0)
        .await;
    assert_eq!(oauth.requests.lock().unwrap().len(), 1);
    assert!(host.requests.lock().unwrap().is_empty());
    pair_fixture_cli(&mut cli).await;
    cli.enter_conversation().await;
    assert_eq!(
        oauth.requests.lock().unwrap().len(),
        1,
        "no warmup model invocation"
    );
    cli.send(&format!("{OAUTH_PROMPT}\n"));
    cli.shown(OAUTH_ANSWER, 0).await;
    let completed = cli.transcript.len();
    // The final answer and next prompt may have arrived in one read.
    if !cli
        .transcript
        .split(OAUTH_ANSWER)
        .last()
        .unwrap()
        .contains("You: ")
    {
        cli.shown("You: ", completed).await;
    }
    cli.send("/quit\n");
    assert!(cli.finish().await.success());
    assert_provider_callback_retired(&redirect).await;
    host.finish().await;
    oauth.finish().await;
    assert_eq!(
        host.requests
            .lock()
            .unwrap()
            .iter()
            .map(|request| (request.method.as_str(), request.path.as_str()))
            .collect::<Vec<_>>(),
        vec![
            ("POST", "/console/pair/request"),
            ("POST", "/console/pair/confirm"),
        ],
        "one protected host pairing and no frontend request"
    );
    assert_eq!(
        oauth.requests.lock().unwrap().len(),
        2,
        "one exchange then one model request"
    );
    assert_eq!(cli.transcript.matches("Setup session: ").count(), 1);
    assert_eq!(
        cli.transcript
            .matches("Open this private sign-in URL")
            .count(),
        1
    );
    cli.assert_reviewed_setup(&config);
    assert_eq!(preparation_output_count(&cli.state), 1);

    // Reopen only this child's native storage after the executable has joined.
    let session_id = meerkat_core::SessionId::parse(
        cli.transcript
            .split("Setup session: ")
            .nth(1)
            .unwrap()
            .split_whitespace()
            .next()
            .unwrap(),
    )
    .unwrap();
    let root = cli._root.as_ref().unwrap().path();
    let storage = meerkat_connect::RealmOptions {
        context_root: root.join("outside-checkout"),
        state_root: Some(cli.state.clone()),
        user_config_root: Some(root.join("home")),
    }
    .resolve(REALM)
    .unwrap();
    let (_, persistence) = timeout(
        BOUND,
        meerkat::governed_realm::open_stock_realm_persistence_as(
            &meerkat::DiskStorageProvider,
            &storage.locator.state_root,
            REALM,
            None,
            Some(meerkat_store::RealmOrigin::Explicit),
            Some(storage.layout),
            meerkat::governed_realm::stock::issuance::DEFAULT_STOCK_AGENT,
        ),
    )
    .await
    .unwrap()
    .unwrap();
    let session = timeout(BOUND, persistence.session_store().load(&session_id))
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let history = serde_json::to_string(session.messages()).unwrap();
    assert!(history.contains(OAUTH_PROMPT) && history.contains(OAUTH_ANSWER));
    for private in [
        &state,
        OAUTH_CODE,
        OAUTH_TOKEN,
        OAUTH_REFRESH,
        CODE,
        DEVICE,
        PAIR,
    ] {
        assert!(
            !history.contains(private),
            "private authentication value entered native history"
        );
    }
}

#[tokio::test]
async fn actual_cli_configured_managed_api_key_is_hidden_and_reaches_native_model() {
    use meerkat_auth_core::auth_store::{FileTokenStore, PersistedAuthMode, TokenKey, TokenStore};
    let mut host = Server::start(false).await;
    let mut provider = OAuthFixture::start_managed_api_key().await;
    let mut cli = Cli::spawn_oauth(&host.origin, &provider);
    let config = fs::read(cli.state.join(REALM).join("config.toml")).unwrap();
    cli.shown("New realm: ", 0).await;
    cli.send(&format!("{REALM}\n"));
    cli.approve_setup_helper().await;
    cli.shown("Model target (number, or /quit): ", 0).await;
    cli.send("1\n");
    cli.shown("Type 'enter-key'", 0).await;
    cli.send("enter-key\n");
    assert!(
        cli.transcript
            .contains("owner realm: cli-frontend; binding: browser; profile: browser")
    );
    assert!(host.requests.lock().unwrap().is_empty());
    assert!(provider.requests.lock().unwrap().is_empty());
    cli.shown("Enter the API key (hidden)", 0).await;
    assert!(
        !tcgetattr(&cli.observer)
            .unwrap()
            .local_flags
            .contains(LocalFlags::ECHO)
    );
    cli.send(&format!("{OAUTH_TOKEN}\n"));
    cli.shown("API key stored by the native credential owner.", 0)
        .await;
    pair_fixture_cli(&mut cli).await;
    cli.enter_conversation().await;
    assert!(
        provider.requests.lock().unwrap().is_empty(),
        "no provider validation/warmup request"
    );
    cli.send(&format!("{OAUTH_PROMPT}\n"));
    cli.shown(OAUTH_ANSWER, 0).await;
    if !cli
        .transcript
        .split(OAUTH_ANSWER)
        .last()
        .unwrap()
        .contains("You: ")
    {
        let offset = cli.transcript.len();
        cli.shown("You: ", offset).await;
    }
    cli.send("/quit\n");
    assert!(cli.finish().await.success());
    host.finish().await;
    provider.finish().await;
    assert_eq!(
        provider.requests.lock().unwrap().len(),
        1,
        "one actual model call, no key exchange/retry"
    );
    assert_eq!(
        host.requests.lock().unwrap().len(),
        2,
        "one host pairing only"
    );
    assert!(!cli.transcript.contains(OAUTH_TOKEN));
    assert_eq!(cli.transcript.matches("Setup session: ").count(), 1);
    cli.assert_reviewed_setup(&config);
    let root = cli._root.as_ref().unwrap().path();
    let home = root.join("home");
    #[cfg(target_os = "macos")]
    let config_dir = home.join("Library/Application Support");
    #[cfg(not(target_os = "macos"))]
    let config_dir = home.join("native-config");
    let store = FileTokenStore::new(config_dir.join("meerkat/credentials"));
    let key = TokenKey::parse(REALM, "browser").unwrap();
    assert_eq!(store.list().await.unwrap(), vec![key.clone()]);
    let saved = store.load(&key).await.unwrap().unwrap();
    assert_eq!(saved.auth_mode, PersistedAuthMode::ApiKey);
    assert!(saved.primary_secret.as_deref() == Some(OAUTH_TOKEN));
    let session_id = meerkat_core::SessionId::parse(
        cli.transcript
            .split("Setup session: ")
            .nth(1)
            .unwrap()
            .split_whitespace()
            .next()
            .unwrap(),
    )
    .unwrap();
    let storage = meerkat_connect::RealmOptions {
        context_root: root.join("outside-checkout"),
        state_root: Some(cli.state.clone()),
        user_config_root: Some(home),
    }
    .resolve(REALM)
    .unwrap();
    let (_, persistence) = meerkat::governed_realm::open_stock_realm_persistence_as(
        &meerkat::DiskStorageProvider,
        &storage.locator.state_root,
        REALM,
        None,
        Some(meerkat_store::RealmOrigin::Explicit),
        Some(storage.layout),
        meerkat::governed_realm::stock::issuance::DEFAULT_STOCK_AGENT,
    )
    .await
    .unwrap();
    let session = persistence
        .session_store()
        .load(&session_id)
        .await
        .unwrap()
        .unwrap();
    let history = serde_json::to_string(session.messages()).unwrap();
    assert!(history.contains(OAUTH_PROMPT) && history.contains(OAUTH_ANSWER));
    for private in [OAUTH_TOKEN, CODE, DEVICE, PAIR] {
        assert!(
            !history.contains(private),
            "protected input must not enter native history"
        );
    }
    assert_eq!(preparation_output_count(&cli.state), 1);
}

#[tokio::test]
async fn actual_cli_provider_callback_cancel_joins_without_pairing_or_exchange() {
    let mut host = Server::start(false).await;
    let mut oauth = OAuthFixture::start(false, false).await;
    let mut cli = Cli::spawn_oauth(&host.origin, &oauth);
    let config = fs::read(cli.state.join(REALM).join("config.toml")).unwrap();
    let (redirect, state) = begin_provider_login(&mut cli, &oauth, &host).await;
    killpg(Pid::from_raw(cli.child.id() as i32), Signal::SIGINT).unwrap();
    cli.shown("Native callback cleanup is pending", 0).await;
    assert!(
        cli.child.try_wait().unwrap().is_none(),
        "cancellation still owns callback cleanup"
    );
    deliver_provider_callback(&redirect, &state, false).await;
    assert!(cli.finish().await.success());
    assert_provider_callback_retired(&redirect).await;
    host.finish().await;
    oauth.finish().await;
    assert!(host.requests.lock().unwrap().is_empty());
    assert!(oauth.requests.lock().unwrap().is_empty());
    assert!(!cli.transcript.contains("Setup session: "));
    assert!(!cli.transcript.contains("Type 'request'"));
    cli.assert_reviewed_setup(&config);
}

#[tokio::test]
async fn actual_cli_provider_exchange_cancel_waits_for_native_settlement_without_pairing() {
    let mut host = Server::start(false).await;
    let mut oauth = OAuthFixture::start(true, false).await;
    let mut cli = Cli::spawn_oauth(&host.origin, &oauth);
    let config = fs::read(cli.state.join(REALM).join("config.toml")).unwrap();
    let (redirect, state) = begin_provider_login(&mut cli, &oauth, &host).await;
    deliver_provider_callback(&redirect, &state, false).await;
    oauth.exchange_received().await;
    killpg(Pid::from_raw(cli.child.id() as i32), Signal::SIGINT).unwrap();
    cli.shown("Waiting for native credential exchange to settle", 0)
        .await;
    assert!(
        cli.child.try_wait().unwrap().is_none(),
        "native mutation is still owned"
    );
    assert!(host.requests.lock().unwrap().is_empty());
    oauth.release_exchange.add_permits(1);
    cli.shown("Native sign-in completed for the selected owner", 0)
        .await;
    assert!(cli.finish().await.success());
    assert_provider_callback_retired(&redirect).await;
    host.finish().await;
    oauth.finish().await;
    assert_eq!(
        oauth.requests.lock().unwrap().len(),
        1,
        "no exchange replay or model request"
    );
    assert!(host.requests.lock().unwrap().is_empty());
    assert!(!cli.transcript.contains("Setup session: "));
    assert!(!cli.transcript.contains("Type 'request'"));
    cli.assert_reviewed_setup(&config);
}

#[tokio::test]
async fn actual_cli_provider_exchange_failure_redacts_and_never_retries() {
    let mut host = Server::start(false).await;
    let mut oauth = OAuthFixture::start(false, true).await;
    let mut cli = Cli::spawn_oauth(&host.origin, &oauth);
    let (redirect, state) = begin_provider_login(&mut cli, &oauth, &host).await;
    deliver_provider_callback(&redirect, &state, false).await;
    assert!(!cli.finish().await.success());
    assert_provider_callback_retired(&redirect).await;
    host.finish().await;
    oauth.finish().await;
    assert!(
        cli.transcript
            .contains("Complete native sign-in failed; provider details are withheld")
    );
    assert!(!cli.transcript.contains("Native sign-in completed"));
    assert_eq!(oauth.requests.lock().unwrap().len(), 1);
    assert!(host.requests.lock().unwrap().is_empty());
    assert!(!cli.transcript.contains("Setup session: "));
    cli.assert_reviewed_setup(&cli.initial_config);
}

async fn startup_cancel_during_refresh(fail_refresh: bool) {
    use meerkat_auth_core::auth_store::{FileTokenStore, TokenKey, TokenStore};

    let mut host = Server::start(false).await;
    let mut oauth = OAuthFixture::start_login_then_refresh(fail_refresh).await;
    let mut cli = Cli::spawn_configured(None, &oauth.origin, None, false, Some(&oauth), None);
    let root = cli._root.as_ref().unwrap().path().to_owned();
    let home = root.join("home");
    #[cfg(target_os = "macos")]
    let config_dir = home.join("Library/Application Support");
    #[cfg(not(target_os = "macos"))]
    let config_dir = home.join("native-config");
    let store = FileTokenStore::new(config_dir.join("meerkat/credentials"));
    let config = fs::read(cli.state.join(REALM).join("config.toml")).unwrap();
    let native_config =
        meerkat_core::Config::from_persisted_toml(std::str::from_utf8(&config).unwrap())
            .unwrap()
            .0;
    let target = meerkat_core::resolve_explicit_auth_binding_target(
        &native_config,
        &meerkat_core::AuthBindingRef {
            realm: meerkat_core::RealmId::parse(REALM).unwrap(),
            binding: meerkat_core::BindingId::parse("browser").unwrap(),
            profile: None,
            origin: meerkat_core::BindingOrigin::Configured,
        },
    )
    .unwrap();
    let key = TokenKey::from_credential_identity(&target.credential_identity);
    // The real protected login publishes the native credential and lifecycle;
    // the fixture supplies a short expiry rather than writing store metadata.
    let (redirect, state) = begin_provider_login(&mut cli, &oauth, &host).await;
    deliver_provider_callback(&redirect, &state, false).await;
    cli.shown("Native sign-in completed for the selected owner", 0)
        .await;
    assert_provider_callback_retired(&redirect).await;
    oauth.grant_received("refresh_token").await;
    cli.drain();
    assert!(cli.transcript.contains(SETUP_HELPER_REVIEW));
    assert!(!cli.transcript.contains(SETUP_MODEL_REVIEW));
    assert_eq!(cli.transcript.matches("Model target (number").count(), 1);
    assert_eq!(
        cli.transcript
            .matches("Open this private sign-in URL")
            .count(),
        1
    );
    assert!(!cli.transcript.contains("You: "));

    killpg(Pid::from_raw(cli.child.id() as i32), Signal::SIGINT).unwrap();
    cli.shown("Waiting for native setup startup to settle", 0)
        .await;
    assert!(
        cli.child.try_wait().unwrap().is_none(),
        "native refresh remains owned"
    );
    assert_eq!(oauth.requests.lock().unwrap().len(), 2);
    oauth.release_exchange.add_permits(1);
    let status = cli.finish().await;
    oauth.finish().await;
    host.finish().await;
    assert!(host.requests.lock().unwrap().is_empty());
    assert_eq!(status.success(), !fail_refresh);
    assert!(!cli.transcript.contains("Setup session: "));
    assert!(!cli.transcript.contains("You: "));
    assert!(!cli.transcript.contains("Type 'request'"));
    assert_eq!(preparation_output_count(&cli.state), 1);
    cli.assert_reviewed_setup(&config);
    {
        let requests = oauth.requests.lock().unwrap();
        assert_eq!(
            requests
                .iter()
                .map(|request| (
                    request.path.as_str(),
                    request.body["grant_type"].as_str().unwrap()
                ))
                .collect::<Vec<_>>(),
            vec![
                ("/anthropic/token", "authorization_code"),
                ("/anthropic/token", "refresh_token"),
            ],
            "one login and one refresh, no model call or retry"
        );
    }
    if fail_refresh {
        assert!(cli.transcript.contains("Prepare actual setup controller"));
    } else {
        let stored = timeout(BOUND, store.load(&key))
            .await
            .unwrap()
            .unwrap()
            .expect("settled native refresh remains stored after cancellation");
        assert!(stored.primary_secret.as_deref() == Some(OAUTH_TOKEN));
        assert!(stored.refresh_token.as_deref() == Some(OAUTH_REFRESH));
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        assert!(stored.expires_at.unwrap().timestamp() > (now + 60) as i64);
    }

    // Reopen only after the actual child and its process group have joined.
    // Read native session/input records without constructing a replacement agent.
    let storage = meerkat_connect::RealmOptions {
        context_root: root.join("outside-checkout"),
        state_root: Some(cli.state.clone()),
        user_config_root: Some(home),
    }
    .resolve(REALM)
    .unwrap();
    let (_, persistence) = timeout(
        BOUND,
        meerkat::governed_realm::open_stock_realm_persistence_as(
            &meerkat::DiskStorageProvider,
            &storage.locator.state_root,
            REALM,
            None,
            Some(meerkat_store::RealmOrigin::Explicit),
            Some(storage.layout),
            meerkat::governed_realm::stock::issuance::DEFAULT_STOCK_AGENT,
        ),
    )
    .await
    .unwrap()
    .unwrap();
    let sessions = timeout(
        BOUND,
        persistence
            .session_store()
            .list(meerkat_core::SessionFilter {
                limit: Some(2),
                ..Default::default()
            }),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(
        sessions.is_empty(),
        "cancelled preparation must not materialize a session"
    );
    // Inspect the actual session selected before review, even though its actor
    // never materialized and no post-materialization banner was printed.
    let inputs = timeout(
        BOUND,
        persistence.runtime_store().load_input_states_strict(
            &meerkat_runtime::identifiers::LogicalRuntimeId::for_session(&cli.reviewed_session()),
        ),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(
        inputs.is_empty(),
        "no initial input may be admitted after cancellation"
    );
}

#[tokio::test]
async fn actual_cli_startup_cancel_joins_native_refresh_and_closes_without_input() {
    startup_cancel_during_refresh(false).await;
}

#[tokio::test]
async fn actual_cli_startup_cancel_preserves_native_refresh_failure_without_input() {
    startup_cancel_during_refresh(true).await;
}

async fn select_device_target(cli: &mut Cli, device: &DeviceFixture, host: &Server) {
    cli.shown("New realm: ", 0).await;
    cli.send(&format!("{REALM}\n"));
    cli.approve_setup_helper().await;
    cli.shown("Model target (number, or /quit): ", 0).await;
    assert!(
        cli.transcript
            .contains("Owner realm: cli-frontend; binding: copilot")
    );
    cli.send("1\n");
    cli.shown("Type 'signin' to authenticate this exact owner", 0)
        .await;
    assert!(device.calls.lock().unwrap().is_empty());
    assert!(host.requests.lock().unwrap().is_empty());
    assert!(!cli.transcript.contains("Type 'request'"));
    assert!(!cli.transcript.contains("Setup session: "));
}

async fn assert_device_owner_storage(cli: &Cli, expected: bool) {
    use meerkat_auth_core::auth_store::{FileTokenStore, PersistedAuthMode, TokenKey, TokenStore};
    // These are the child's isolated native default directories, not a store
    // selected by Connect. Read only through the released native store API.
    let home = cli._root.as_ref().unwrap().path().join("home");
    #[cfg(target_os = "macos")]
    let config_dir = home.join("Library/Application Support");
    #[cfg(not(target_os = "macos"))]
    let config_dir = home.join("native-config");
    let store = FileTokenStore::new(config_dir.join("meerkat/credentials"));
    let text = fs::read_to_string(cli.state.join(REALM).join("config.toml")).unwrap();
    let config = meerkat_core::Config::from_persisted_toml(&text).unwrap().0;
    let target = meerkat_core::resolve_explicit_auth_binding_target(
        &config,
        &meerkat_core::AuthBindingRef {
            realm: meerkat_core::RealmId::parse(REALM).unwrap(),
            binding: meerkat_core::BindingId::parse("copilot").unwrap(),
            profile: None,
            origin: meerkat_core::BindingOrigin::Configured,
        },
    )
    .unwrap();
    let key = TokenKey::from_credential_identity(&target.credential_identity);
    assert_eq!(key.realm().as_str(), REALM);
    assert_eq!(key.account().unwrap().as_str(), "github_copilot");
    let keys = timeout(BOUND, store.list()).await.unwrap().unwrap();
    if expected {
        assert_eq!(
            keys,
            vec![key.clone()],
            "only the selected native account owns a stored credential"
        );
        let stored = timeout(BOUND, store.load(&key))
            .await
            .unwrap()
            .unwrap()
            .expect("native published credential");
        assert_eq!(stored.auth_mode, PersistedAuthMode::GithubCopilotOauth);
        assert!(
            stored.primary_secret.as_deref() == Some(OAUTH_TOKEN),
            "native stored the synthetic token without copying it to a binding"
        );
    } else {
        assert!(
            keys.is_empty(),
            "canceled unpublished setup must not create credentials"
        );
    }
}

fn assert_device_stopped_before_setup(cli: &Cli, host: &Server, config: &[u8]) {
    assert!(host.requests.lock().unwrap().is_empty());
    assert!(!cli.transcript.contains("Setup session: "));
    assert!(!cli.transcript.contains("You: "));
    cli.assert_reviewed_setup(config);
}

#[tokio::test]
async fn actual_cli_device_login_paces_polls_persists_native_account_then_reaches_pairing() {
    let mut host = Server::start(false).await;
    let mut device = DeviceFixture::start(DeviceScenario::Progress).await;
    let mut cli = Cli::spawn_device(&host.origin, &device);
    let config = fs::read(cli.state.join(REALM).join("config.toml")).unwrap();
    select_device_target(&mut cli, &device, &host).await;
    cli.send("signin\n");
    cli.shown("Waiting for native device authorization", 0)
        .await;
    assert!(
        cli.transcript
            .contains(&format!("{}/human-device-verification", device.origin))
    );
    assert!(
        cli.transcript
            .contains(&format!("Enter this user code: {COPILOT_USER_CODE}"))
    );
    cli.shown("Native device sign-in completed for the selected owner", 0)
        .await;
    cli.shown("Type 'request'", 0).await;
    cli.send("/quit\n");
    assert!(cli.finish().await.success());
    host.finish().await;
    device.finish().await;
    {
        let calls = device.calls.lock().unwrap();
        assert_eq!(
            calls
                .iter()
                .map(|(path, _)| path.as_str())
                .collect::<Vec<_>>(),
            vec![
                "/copilot/device/code",
                "/copilot/token",
                "/copilot/token",
                "/copilot/token",
            ]
        );
        for (index, minimum) in [(1, 1), (2, 1), (3, 6)] {
            assert!(
                calls[index].1.duration_since(calls[index - 1].1) >= Duration::from_secs(minimum),
                "native interval and cumulative SlowDown must delay each next poll"
            );
        }
    }
    assert_eq!(
        cli.transcript
            .matches("Open this private device sign-in page:")
            .count(),
        1
    );
    assert_device_stopped_before_setup(&cli, &host, &config);
    assert_device_owner_storage(&cli, true).await;
}

#[tokio::test]
async fn actual_cli_device_cancel_before_start_sends_no_auth_request() {
    let mut host = Server::start(false).await;
    let mut device = DeviceFixture::start(DeviceScenario::Wait).await;
    let mut cli = Cli::spawn_device(&host.origin, &device);
    let config = fs::read(cli.state.join(REALM).join("config.toml")).unwrap();
    select_device_target(&mut cli, &device, &host).await;
    killpg(Pid::from_raw(cli.child.id() as i32), Signal::SIGINT).unwrap();
    assert!(cli.finish().await.success());
    host.finish().await;
    device.finish().await;
    assert!(device.calls.lock().unwrap().is_empty());
    assert!(!cli.transcript.contains("Type 'request'"));
    assert_device_stopped_before_setup(&cli, &host, &config);
    assert_device_owner_storage(&cli, false).await;
}

#[tokio::test]
async fn actual_cli_device_cancel_during_interval_stops_without_polling_or_pairing() {
    let mut host = Server::start(false).await;
    let mut device = DeviceFixture::start(DeviceScenario::Wait).await;
    let mut cli = Cli::spawn_device(&host.origin, &device);
    let config = fs::read(cli.state.join(REALM).join("config.toml")).unwrap();
    select_device_target(&mut cli, &device, &host).await;
    cli.send("signin\n");
    cli.shown("Waiting for native device authorization", 0)
        .await;
    killpg(Pid::from_raw(cli.child.id() as i32), Signal::SIGINT).unwrap();
    cli.shown("Device sign-in polling stopped.", 0).await;
    assert!(cli.finish().await.success());
    host.finish().await;
    device.finish().await;
    assert_eq!(
        device
            .calls
            .lock()
            .unwrap()
            .iter()
            .map(|(path, _)| path.clone())
            .collect::<Vec<_>>(),
        vec!["/copilot/device/code"]
    );
    assert!(!cli.transcript.contains("Type 'request'"));
    assert_device_stopped_before_setup(&cli, &host, &config);
    assert_device_owner_storage(&cli, false).await;
}

#[tokio::test]
async fn actual_cli_device_cancel_during_ready_poll_joins_storage_without_pairing() {
    let mut host = Server::start(false).await;
    let mut device = DeviceFixture::start(DeviceScenario::HeldReady).await;
    let mut cli = Cli::spawn_device(&host.origin, &device);
    let config = fs::read(cli.state.join(REALM).join("config.toml")).unwrap();
    select_device_target(&mut cli, &device, &host).await;
    cli.send("signin\n");
    device.poll_received().await;
    killpg(Pid::from_raw(cli.child.id() as i32), Signal::SIGINT).unwrap();
    cli.shown("Waiting for the native device request to settle", 0)
        .await;
    assert!(
        cli.child.try_wait().unwrap().is_none(),
        "the in-flight native poll remains owned"
    );
    assert!(host.requests.lock().unwrap().is_empty());
    device.release.add_permits(1);
    cli.shown("Native device sign-in completed for the selected owner", 0)
        .await;
    cli.shown("Device sign-in polling stopped.", 0).await;
    assert!(cli.finish().await.success());
    host.finish().await;
    device.finish().await;
    assert_eq!(
        device
            .calls
            .lock()
            .unwrap()
            .iter()
            .map(|(path, _)| path.clone())
            .collect::<Vec<_>>(),
        vec!["/copilot/device/code", "/copilot/token"]
    );
    assert!(!cli.transcript.contains("Type 'request'"));
    assert_device_stopped_before_setup(&cli, &host, &config);
    assert_device_owner_storage(&cli, true).await;
}

#[test]
fn memory_only_requires_an_explicit_paired_host_origin() {
    let output = Command::new(env!("CARGO_BIN_EXE_meerkat-connect"))
        .arg("--paired-host-memory-only")
        .env_clear()
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("--paired-host"));
}

#[tokio::test]
async fn actual_cli_reads_service_recipes_without_resources_in_original_session() {
    recipe_discovery_cli(false, false, false).await;
}

#[tokio::test]
async fn actual_cli_names_new_realm_with_existing_inventory_and_preserves_old_realm() {
    recipe_discovery_cli(true, false, false).await;
}

#[tokio::test]
async fn actual_cli_failed_read_turn_accepts_only_explicit_followup_in_same_session() {
    recipe_discovery_cli(false, true, false).await;
}

#[tokio::test]
async fn actual_cli_refuses_private_document_and_keeps_original_conversation() {
    recipe_discovery_cli(false, false, true).await;
}

#[path = "cli_paired_host/profile_denial.rs"]
mod profile_denial;

/// Uses the real supplied package, checked installation and pinned verifier.
/// No installed receipt, native connector entry or grant is fabricated.
#[tokio::test]
#[ignore = "requires isolated supplied resources and exact pin-built CLI"]
async fn actual_cli_denied_client_profile_keeps_checked_install_turn_and_recipe_sibling() {
    profile_denial::run().await;
}

#[path = "cli_paired_host/public_service.rs"]
mod public_service;

/// Actual supplied Agno package, live public HTTPS, then a fresh ordinary rkat.
/// Explicit invocation must supply the exact final native binary and its hash.
#[tokio::test]
#[ignore = "requires supplied library, exact-final rkat and live public Hacker News"]
async fn actual_cli_public_service_survives_setup_exit_and_fresh_rkat_in_exact_realm() {
    public_service::run().await;
}

async fn root_with_existing_realm() -> tempfile::TempDir {
    // Establish a genuine native manifest and persisted transcript before the
    // CLI starts. The picker must only read this unrelated realm's manifest.
    let root = tempfile::tempdir().unwrap();
    let state = root.path().join("state");
    let storage = meerkat_connect::RealmOptions {
        context_root: root.path().to_owned(),
        state_root: Some(state.clone()),
        user_config_root: Some(root.path().join("home")),
    }
    .resolve(EXISTING_REALM)
    .unwrap();
    let (_, persistence) = timeout(
        BOUND,
        meerkat::storage_provider::open_realm_persistence_with_layout(
            storage.layout,
            EXISTING_REALM,
            None,
            Some(meerkat_store::RealmOrigin::Explicit),
        ),
    )
    .await
    .unwrap()
    .unwrap();
    let mut history = meerkat_core::Session::new();
    history.push(meerkat_core::Message::User(
        meerkat_core::UserMessage::text(EXISTING_REALM_HISTORY),
    ));
    timeout(BOUND, persistence.session_store().save(&history))
        .await
        .unwrap()
        .unwrap();
    let stored = timeout(BOUND, persistence.session_store().load(history.id()))
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(stored.messages(), history.messages());
    drop(persistence);
    fs::write(
        state.join(EXISTING_REALM).join("config.toml"),
        "[agent]\nmax_turns = 3\n",
    )
    .unwrap();
    root
}

fn existing_realm_files(state: &Path) -> BTreeMap<PathBuf, String> {
    let root = state.join(EXISTING_REALM);
    let mut pending = vec![root.clone()];
    let mut files = BTreeMap::new();
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(directory).unwrap() {
            let path = entry.unwrap().path();
            let metadata = fs::symlink_metadata(&path).unwrap();
            assert!(!metadata.file_type().is_symlink());
            if metadata.is_dir() {
                pending.push(path);
            } else {
                assert!(metadata.is_file());
                files.insert(
                    path.strip_prefix(&root).unwrap().to_owned(),
                    file_digest(&path),
                );
            }
        }
    }
    assert!(files.contains_key(Path::new("realm_manifest.json")));
    assert!(files.contains_key(Path::new("config.toml")));
    assert!(files.len() > 2, "native persisted history must be included");
    files
}

async fn recipe_discovery_cli(existing_inventory: bool, failed_turn: bool, document_refusal: bool) {
    let mut provider = if document_refusal {
        Server::start_document_refusal().await
    } else if failed_turn {
        Server::start_recipe_failure().await
    } else {
        Server::start_recipe_discovery().await
    };
    let existing_root = if existing_inventory {
        Some(root_with_existing_realm().await)
    } else {
        None
    };
    let previous = existing_root
        .as_ref()
        .map(|root| existing_realm_files(&root.path().join("state")));
    let mut cli = match existing_root {
        Some(root) => {
            Cli::spawn_configured_in(root, None, &provider.origin, None, false, None, None)
        }
        None => Cli::spawn_selected(None, &provider.origin, None),
    };
    let config = fs::read(cli.state.join(REALM).join("config.toml")).unwrap();
    if existing_inventory {
        cli.shown("Choose a realm number, 'new', or /quit: ", 0)
            .await;
        assert!(cli.transcript.contains(&format!("1. {EXISTING_REALM} (")));
        assert!(!cli.state.join(REALM).join("realm_manifest.json").exists());
        cli.send("new\n");
        cli.shown("New realm: ", 0).await;
        cli.send(&format!("{EXISTING_REALM}\n"));
        cli.shown("That realm is already listed at this root.", 0)
            .await;
        let retry = cli
            .transcript
            .find("That realm is already listed at this root.")
            .unwrap();
        cli.shown("Choose a realm number, 'new', or /quit: ", retry)
            .await;
        let naming = cli.transcript.len();
        cli.send("new\n");
        cli.shown("New realm: ", naming).await;
    }
    cli.shown("New realm: ", 0).await;
    if existing_inventory {
        cli.send("not a valid realm\n");
        cli.shown("That name is not valid", 0).await;
        assert!(!cli.state.join(REALM).join("realm_manifest.json").exists());
        assert!(provider.requests.lock().unwrap().is_empty());
    }
    cli.send(&format!("{REALM}\n"));
    cli.approve_setup_helper().await;
    cli.enter_conversation().await;
    assert!(provider.requests.lock().unwrap().is_empty());
    let first_turn = cli.transcript.len();
    cli.send(&format!("{RECIPE_REQUEST}\n"));
    cli.shown(RECIPE_ANSWER, first_turn).await;
    cli.shown("You: ", first_turn).await;
    if document_refusal {
        let reading = cli.transcript.len();
        cli.send(&format!("{DOCUMENT_REQUEST}\n"));
        cli.shown(DOCUMENT_ANSWER, reading).await;
        cli.shown("You: ", reading).await;
        assert_eq!(provider.requests.lock().unwrap().len(), 6);
    }
    if failed_turn {
        assert_eq!(provider.requests.lock().unwrap().len(), 4);
        let failure = cli.transcript.len();
        cli.send(&format!("{RECIPE_FAILED_REQUEST}\n"));
        cli.shown(
            "The native setup turn failed. No automatic retry was started.",
            failure,
        )
        .await;
        cli.shown("You: ", failure).await;
        assert_eq!(provider.requests.lock().unwrap().len(), 5);
        assert!(!cli.transcript[failure..].contains(RECIPE_ANSWER));
        assert!(!cli.transcript[failure..].contains(RECIPE_FOLLOWUP_ANSWER));
        assert!(!cli.transcript.contains(RECIPE_PROVIDER_FAILURE));
        assert!(cli.child.try_wait().unwrap().is_none());
    }
    let followup = cli.transcript.len();
    cli.send(&format!("{RECIPE_FOLLOWUP}\n"));
    cli.shown(RECIPE_FOLLOWUP_ANSWER, followup).await;
    cli.shown("You: ", followup).await;
    cli.send("/quit\n");
    assert!(cli.finish().await.success(), "{}", cli.transcript);
    provider.finish().await;
    assert_eq!(
        provider.requests.lock().unwrap().len(),
        if document_refusal {
            7
        } else if failed_turn {
            6
        } else {
            5
        },
    );
    assert_eq!(cli.transcript.matches("Setup session: ").count(), 1);
    for marker in ["Paired host origin: ", "Enter the six-digit code"] {
        assert!(!cli.transcript.contains(marker));
    }
    assert_eq!(cli.transcript.matches("Type 'approve ").count(), 2);
    assert!(cli.prefix.join("libexec").exists());
    assert_eq!(preparation_output_count(&cli.state), 1);
    cli.assert_reviewed_setup(&config);
    if let Some(previous) = previous {
        assert_eq!(existing_realm_files(&cli.state), previous);
    }

    // Read the exact persisted session only after the copied executable joins.
    let session_id = meerkat_core::SessionId::parse(
        cli.transcript
            .split("Setup session: ")
            .nth(1)
            .unwrap()
            .split_whitespace()
            .next()
            .unwrap(),
    )
    .unwrap();
    let root = cli._root.as_ref().unwrap().path();
    let storage = meerkat_connect::RealmOptions {
        context_root: root.join("outside-checkout"),
        state_root: Some(cli.state.clone()),
        user_config_root: Some(root.join("home")),
    }
    .resolve(REALM)
    .unwrap();
    let (_, persistence) = timeout(
        BOUND,
        meerkat::governed_realm::open_stock_realm_persistence_as(
            &meerkat::DiskStorageProvider,
            &storage.locator.state_root,
            REALM,
            None,
            Some(meerkat_store::RealmOrigin::Explicit),
            Some(storage.layout),
            meerkat::governed_realm::stock::issuance::DEFAULT_STOCK_AGENT,
        ),
    )
    .await
    .unwrap()
    .unwrap();
    let session = timeout(BOUND, persistence.session_store().load(&session_id))
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let history = serde_json::to_string(session.messages()).unwrap();
    for text in [
        RECIPE_REQUEST,
        RECIPE_ANSWER,
        RECIPE_FOLLOWUP,
        RECIPE_FOLLOWUP_ANSWER,
    ] {
        assert!(history.contains(text));
    }
    assert!(history.contains("recipe-exact"));
    for protected in [CODE, DEVICE, PAIR, TOKEN] {
        assert!(!history.contains(protected));
    }
    if failed_turn || document_refusal {
        use meerkat_runtime::{
            completion::CompletionOutcome,
            identifiers::LogicalRuntimeId,
            input_state::{InputLifecycleState, InputTerminalOutcome},
        };
        assert!(history.contains(if failed_turn {
            RECIPE_FAILED_REQUEST
        } else {
            DOCUMENT_REQUEST
        }));
        if document_refusal {
            assert!(history.contains(DOCUMENT_ANSWER));
        }
        assert_eq!(
            cli.transcript
                .matches("The native setup turn failed.")
                .count(),
            usize::from(failed_turn)
        );
        for request in provider.requests.lock().unwrap().iter() {
            for protected in [CODE, DEVICE, PAIR, TOKEN] {
                assert!(!request.body.to_string().contains(protected));
            }
        }
        let mut inputs = timeout(
            BOUND,
            persistence
                .runtime_store()
                .load_input_states_strict(&LogicalRuntimeId::for_session(&session_id)),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(
            inputs.len(),
            3,
            "one native input per explicit human message"
        );
        inputs.sort_by_key(|input| input.seed.admission_sequence);
        let input_ids: BTreeSet<_> = inputs
            .iter()
            .map(|input| input.state.input_id.to_string())
            .collect();
        assert_eq!(input_ids.len(), 3);
        for (index, input) in inputs.iter().enumerate() {
            assert_eq!(input.seed.attempt_count, 1, "no runtime replay");
            assert_eq!(input.state.recovery_count, 0);
            let stored = serde_json::to_value(input).unwrap();
            let outcome: CompletionOutcome =
                serde_json::from_value(stored["terminal_completion"]["outcome"].clone()).unwrap();
            if index == 1 && failed_turn {
                assert!(matches!(
                    input.seed.phase,
                    InputLifecycleState::Consumed | InputLifecycleState::Abandoned
                ));
                assert!(matches!(
                    input.seed.terminal_outcome.as_ref(),
                    Some(InputTerminalOutcome::Consumed | InputTerminalOutcome::Abandoned { .. })
                ));
                assert!(matches!(
                    outcome,
                    CompletionOutcome::Abandoned { .. }
                        | CompletionOutcome::AbandonedWithError { .. }
                ));
            } else {
                assert_eq!(input.seed.phase, InputLifecycleState::Consumed);
                assert_eq!(
                    input.seed.terminal_outcome,
                    Some(InputTerminalOutcome::Consumed)
                );
                assert!(matches!(outcome, CompletionOutcome::Completed(_)));
            }
        }
    }
}

#[tokio::test]
async fn actual_cli_new_realm_invalid_name_quit_eof_and_interrupt_leave_realms_unchanged() {
    let mut provider = Server::start_recipe_discovery().await;
    for exit in ["quit", "eof", "interrupt"] {
        let root = root_with_existing_realm().await;
        let previous = existing_realm_files(&root.path().join("state"));
        let mut cli =
            Cli::spawn_configured_in(root, None, &provider.origin, None, false, None, None);
        let config = fs::read(cli.state.join(REALM).join("config.toml")).unwrap();
        cli.shown("Choose a realm number, 'new', or /quit: ", 0)
            .await;
        cli.send("new\n");
        cli.shown("New realm: ", 0).await;
        cli.send("../invalid\n");
        cli.shown("That name is not valid", 0).await;
        let corrected_prompt = cli.transcript.find("That name is not valid").unwrap();
        cli.shown("New realm: ", corrected_prompt).await;
        assert!(!cli.state.join(REALM).join("realm_manifest.json").exists());
        match exit {
            "quit" => cli.send("/quit\n"),
            "eof" => cli.send("\u{4}"),
            "interrupt" => {
                cli.send("unfinished-new-realm");
                killpg(Pid::from_raw(cli.child.id() as i32), Signal::SIGINT).unwrap();
            }
            _ => unreachable!(),
        }
        assert!(cli.finish().await.success(), "{}", cli.transcript);
        assert_eq!(existing_realm_files(&cli.state), previous);
        assert_eq!(
            fs::read(cli.state.join(REALM).join("config.toml")).unwrap(),
            config
        );
        assert!(!cli.state.join(REALM).join("realm_manifest.json").exists());
        assert!(!cli.state.join(".meerkat-connect").exists());
        assert_eq!(preparation_output_count(&cli.state), 0);
        assert!(!cli.transcript.contains("Setup session: "));
        assert!(!cli.transcript.contains("You: "));
        assert!(provider.requests.lock().unwrap().is_empty());
    }
    provider.finish().await;
}

#[tokio::test]
async fn actual_cli_denied_setup_helper_review_leaves_native_realm_unmaterialized() {
    let mut provider = Server::start_recipe_discovery().await;
    let root = root_with_existing_realm().await;
    let previous = existing_realm_files(&root.path().join("state"));
    let mut cli = Cli::spawn_configured_in(root, None, &provider.origin, None, false, None, None);
    let config = cli.initial_config.clone();
    let owner = meerkat_store::grant_owner_store_path(&cli.state.join(REALM));
    assert!(!owner.exists());
    assert!(!cli.state.join(REALM).join("realm_manifest.json").exists());
    cli.shown("Choose a realm number, 'new', or /quit: ", 0)
        .await;
    cli.send("new\n");
    cli.shown("New realm: ", 0).await;
    cli.send(&format!("{REALM}\n"));
    cli.deny_startup_review(SETUP_HELPER_REVIEW).await;
    assert!(cli.finish().await.success(), "{}", cli.transcript);
    provider.finish().await;

    assert!(provider.requests.lock().unwrap().is_empty());
    assert_eq!(existing_realm_files(&cli.state), previous);
    assert_eq!(
        fs::read(cli.state.join(REALM).join("config.toml")).unwrap(),
        config
    );
    assert!(
        !owner.exists(),
        "first denial cannot create the native grant owner or issue grants"
    );
    assert!(!owner.with_file_name("stock-grants.json").exists());
    assert!(!cli.state.join(REALM).join("realm_manifest.json").exists());
    assert!(!cli.state.join(REALM).join("config_state.json").exists());
    assert_eq!(cli.transcript.matches("Type 'approve ").count(), 1);
    assert!(cli.setup_approvals.is_empty());
    let artifacts = preparation_artifacts(&cli.state);
    assert_eq!(artifacts.len(), 1);
    assert!(
        !artifacts[0]
            .join("standalone-mcp-grant-request.txt")
            .exists()
    );
    assert!(
        !artifacts[0].join("standalone-stage-abort.txt").exists(),
        "the refused pre-stage review never acquires a stage"
    );
    // finish() reads this exact native Denied record and its selected session
    // owner, while retaining the shared terminal, secret and process checks.
    cli.reviewed_session();
}

#[tokio::test]
async fn actual_cli_denied_setup_model_review_retains_only_helper_effects_and_no_input() {
    use meerkat::governed_realm::stock::issuance::{DEFAULT_STOCK_AGENT, StockGrantFile};

    let mut provider = Server::start_recipe_discovery().await;
    let root = root_with_existing_realm().await;
    let previous = existing_realm_files(&root.path().join("state"));
    let mut cli = Cli::spawn_configured_in(root, None, &provider.origin, None, false, None, None);
    cli.shown("Choose a realm number, 'new', or /quit: ", 0)
        .await;
    cli.send("new\n");
    cli.shown("New realm: ", 0).await;
    cli.send(&format!("{REALM}\n"));
    cli.approve_setup_helper().await;
    cli.shown(SETUP_MODEL_REVIEW, 0).await;

    // At this second protected review, the separately approved helper grant
    // is already settled. Save the actual native selection and owner bytes.
    let owner = meerkat_store::grant_owner_store_path(&cli.state.join(REALM));
    let selection = owner.with_file_name("stock-grants.json");
    let retained: Vec<_> = [
        owner.clone(),
        selection.clone(),
        cli.state.join(REALM).join("realm_manifest.json"),
        cli.state.join(REALM).join("config.toml"),
        cli.state.join(REALM).join("config_state.json"),
    ]
    .into_iter()
    .map(|path| {
        let bytes = fs::read(&path).unwrap();
        (path, bytes)
    })
    .collect();
    let grants: StockGrantFile = serde_json::from_slice(&fs::read(&selection).unwrap()).unwrap();
    assert_eq!(grants.agents.len(), 1);
    let refs = grants.agents.get(DEFAULT_STOCK_AGENT).unwrap();
    assert!(refs.targeted);
    assert!(refs.operation.is_none() && refs.controller.is_none() && refs.catalog.is_none());
    assert!(refs.tool_bundles.is_empty() && refs.pending_cleanup.is_empty());
    assert_eq!(
        refs.mcp.keys().map(String::as_str).collect::<Vec<_>>(),
        ["toolkit-setup"]
    );
    assert!(
        refs.prompt_selection().is_none(),
        "helper access is not a selected controller grant"
    );

    cli.deny_startup_review(SETUP_MODEL_REVIEW).await;
    assert!(cli.finish().await.success(), "{}", cli.transcript);
    provider.finish().await;
    assert!(provider.requests.lock().unwrap().is_empty());
    assert_eq!(existing_realm_files(&cli.state), previous);
    for (path, before) in retained {
        assert_eq!(
            fs::read(&path).unwrap(),
            before,
            "second denial must retain exactly the independently approved helper effects: {}",
            path.display()
        );
    }
    assert_eq!(cli.setup_approvals.len(), 1);
    assert_eq!(cli.transcript.matches("Type 'approve ").count(), 2);
    assert!(!cli.transcript.contains("Setup session: "));
    assert!(!cli.transcript.contains("You: "));
    assert!(cli.transcript.contains(
        "No controller or additional setup grant changed the realm in this setup stage."
    ));
    let artifacts = preparation_artifacts(&cli.state);
    assert_eq!(artifacts.len(), 1);
    let abort = fs::read_to_string(artifacts[0].join("standalone-stage-abort.txt")).unwrap();
    assert!(abort.contains("issuance: NotEntered"));
    // The outer connector facade may still exist at the first abort. Require
    // its actual native classification and matching terminal projection;
    // final release is established by the post-exit governed reopen below.
    if abort.contains("custody: Released") {
        assert!(
            cli.transcript
                .contains("Setup stopped and this realm owner was released.")
        );
    } else {
        assert!(abort.contains("custody: Retained"));
        assert!(
            cli.transcript
                .contains("a native handle still retains this realm owner")
        );
        assert!(
            cli.transcript
                .contains("This does not confirm that credential work has finished")
        );
    }
    assert_eq!(
        fs::read_to_string(artifacts[0].join("standalone-stage-unobserved.txt")).unwrap(),
        "[]"
    );
    assert!(!artifacts[0].join("standalone-stage-finish.txt").exists());
    assert!(fs::read_dir(&artifacts[0]).unwrap().all(|entry| {
        !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with("standalone-stage-grant-")
    }));

    // Reopen through the actual governed path only after the child's natural
    // exit and absent process group. The denied record supplies the exact
    // selected session ID even though no actor or banner was created.
    let session = cli.reviewed_session();
    let root = cli._root.as_ref().unwrap().path();
    let storage = meerkat_connect::RealmOptions {
        context_root: root.join("outside-checkout"),
        state_root: Some(cli.state.clone()),
        user_config_root: Some(root.join("home")),
    }
    .resolve(REALM)
    .unwrap();
    let (_, persistence) = timeout(
        BOUND,
        meerkat::governed_realm::open_stock_realm_persistence_as(
            &meerkat::DiskStorageProvider,
            &storage.locator.state_root,
            REALM,
            None,
            Some(meerkat_store::RealmOrigin::Explicit),
            Some(storage.layout),
            DEFAULT_STOCK_AGENT,
        ),
    )
    .await
    .unwrap()
    .unwrap();
    let sessions = timeout(
        BOUND,
        persistence
            .session_store()
            .list(meerkat_core::SessionFilter {
                limit: Some(2),
                ..Default::default()
            }),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(
        sessions.is_empty(),
        "denied controller review cannot materialize a session"
    );
    let inputs = timeout(
        BOUND,
        persistence.runtime_store().load_input_states_strict(
            &meerkat_runtime::identifiers::LogicalRuntimeId::for_session(&session),
        ),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(
        inputs.is_empty(),
        "no input may be admitted for the exact denied session"
    );
    drop(persistence);
}

#[tokio::test]
async fn actual_cli_default_frontend_uses_the_pinned_reviewed_setup_helper() {
    deferred_cli(DeferredMode::FrontendOnly, None).await;
}

/// Scripted model and host; real copied CLI, native session, pairing and approval.
/// The synthetic built observation proves no live build, activation or voice use.
#[tokio::test]
async fn actual_cli_combined_roles_preserve_inspection_choice_and_exact_approval() {
    use meerkat_core::types::Message;
    use meerkat_runtime::{
        completion::CompletionOutcome,
        identifiers::LogicalRuntimeId,
        input_state::{InputLifecycleState, InputTerminalOutcome},
    };
    let mut host = Server::start_roles(false).await;
    let mut provider = Server::start_roles(true).await;
    let mut cli = Cli::spawn_selected(None, &provider.origin, None);
    let config = fs::read(cli.state.join(REALM).join("config.toml")).unwrap();
    cli.shown("New realm: ", 0).await;
    cli.send(&format!("{REALM}\n"));
    cli.approve_setup_helper().await;
    cli.enter_conversation().await;
    assert!(host.requests.lock().unwrap().is_empty());
    assert!(provider.requests.lock().unwrap().is_empty());
    cli.send(&format!("{ROLE_REQUEST}\n"));
    cli.shown("Paired host origin: ", 0).await;
    assert!(host.requests.lock().unwrap().is_empty());
    cli.send(&format!("{}\n", host.origin));
    cli.shown("Type 'remember'", 0).await;
    cli.send("memory\n");
    pair_fixture_cli(&mut cli).await;
    cli.shown(ROLE_QUESTION, 0).await;
    cli.shown("You: ", cli.transcript.find(ROLE_QUESTION).unwrap())
        .await;
    assert_eq!(tcgetattr(&cli.observer).unwrap(), cli.termios);
    assert_eq!(
        host.requests.lock().unwrap().len(),
        4,
        "pair twice, inventory and inspection only"
    );
    assert_eq!(provider.requests.lock().unwrap().len(), 4);
    assert_eq!(host.submissions(), 0);
    assert_eq!(cli.transcript.matches("Type 'approve ").count(), 2);
    assert_eq!(preparation_output_count(&cli.state), 1);

    let second_turn = cli.transcript.len();
    cli.send(&format!("{ROLE_CHOICE}\n"));
    cli.shown("Type 'approve ", second_turn).await;
    assert_eq!(host.requests.lock().unwrap().len(), 5);
    assert_eq!(host.submissions(), 0);
    let approval = cli
        .transcript
        .split("Type 'approve ")
        .last()
        .unwrap()
        .split('\'')
        .next()
        .unwrap()
        .to_owned();
    cli.send("approve previous-id\n");
    cli.shown("Use the exact approval ID shown above", second_turn)
        .await;
    assert_eq!(host.submissions(), 0);
    assert_eq!(host.requests.lock().unwrap().len(), 5);
    let approved = cli.transcript.len();
    cli.send(&format!("approve {approval}\n"));
    cli.shown(ROLE_ANSWER, approved).await;
    cli.shown("You: ", approved).await;
    let review_path = role_prepared()["review_url"].as_str().unwrap().to_owned();
    assert!(
        cli.transcript
            .contains(&format!("{}{review_path}", host.origin))
    );
    assert_eq!(
        cli.transcript.matches("Paired host change review").count(),
        1
    );
    assert_eq!(cli.transcript.matches("Type 'approve ").count(), 3);
    assert_eq!(host.submissions(), 1);
    assert_eq!(preparation_output_count(&cli.state), 1);
    cli.send("/quit\n");
    assert!(cli.finish().await.success(), "{}", cli.transcript);
    host.finish().await;
    provider.finish().await;
    assert_eq!(cli.transcript.matches("Setup session: ").count(), 1);
    assert_eq!(cli.transcript.matches("Paired host origin: ").count(), 1);
    assert_eq!(
        cli.transcript
            .matches("Enter the six-digit code (hidden)")
            .count(),
        1
    );
    cli.assert_reviewed_setup(&config);
    {
        let host_requests = host.requests.lock().unwrap();
        assert_eq!(host_requests.len(), 7);
        assert_eq!(
            host_requests
                .iter()
                .map(|r| r.method.as_str())
                .collect::<Vec<_>>(),
            ["POST", "POST", "GET", "GET", "POST", "POST", "GET"]
        );
        assert_eq!(
            host_requests[4].body,
            json!({"new_extension":combined_role_proposal(&role_inspection())})
        );
        assert_eq!(
            host_requests[5].body,
            json!({"change_digest":role_prepared()["change"]["change_digest"],"change":role_prepared()["change"]})
        );
        for request in host_requests.iter().skip(2) {
            assert_eq!(
                request.headers["cookie"],
                format!("paired_host_device={DEVICE}")
            );
            assert!(!request.headers.contains_key("authorization"));
            assert!(!request.headers.contains_key("x-host-dev-token"));
            if request.method == "POST" {
                assert_eq!(request.headers["origin"], host.origin);
            }
        }
    }
    {
        let requests = provider.requests.lock().unwrap();
        assert_eq!(requests.len(), 8, "no implicit extra turn or model replay");
        for request in requests.iter() {
            for private in [&host.origin, &review_path, &approval] {
                assert!(!request.body.to_string().contains(private));
            }
        }
    }

    // Open only the joined child's existing store, without starting a runtime.
    let session_id = meerkat_core::SessionId::parse(
        cli.transcript
            .split("Setup session: ")
            .nth(1)
            .unwrap()
            .split_whitespace()
            .next()
            .unwrap(),
    )
    .unwrap();
    let root = cli._root.as_ref().unwrap().path();
    let storage = meerkat_connect::RealmOptions {
        context_root: root.join("outside-checkout"),
        state_root: Some(cli.state.clone()),
        user_config_root: Some(root.join("home")),
    }
    .resolve(REALM)
    .unwrap();
    let (_, persistence) = timeout(
        BOUND,
        meerkat::governed_realm::open_stock_realm_persistence_as(
            &meerkat::DiskStorageProvider,
            &storage.locator.state_root,
            REALM,
            None,
            Some(meerkat_store::RealmOrigin::Explicit),
            Some(storage.layout),
            meerkat::governed_realm::stock::issuance::DEFAULT_STOCK_AGENT,
        ),
    )
    .await
    .unwrap()
    .unwrap();
    let session = timeout(BOUND, persistence.session_store().load(&session_id))
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let conversation: Vec<_> = session
        .messages()
        .iter()
        .filter_map(|message| match message {
            Message::User(user) => Some(("user", user.text_content())),
            Message::BlockAssistant(assistant) if !assistant.to_string().is_empty() => {
                Some(("assistant", assistant.to_string()))
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        conversation,
        [
            ("user", ROLE_REQUEST.into()),
            ("assistant", ROLE_QUESTION.into()),
            ("user", ROLE_CHOICE.into()),
            ("assistant", ROLE_ANSWER.into())
        ]
    );
    let calls: Vec<_> = session
        .messages()
        .iter()
        .filter_map(|message| match message {
            Message::BlockAssistant(assistant) => Some(assistant.tool_calls()),
            _ => None,
        })
        .flatten()
        .map(|call| call.name.to_owned())
        .collect();
    assert_eq!(
        calls,
        [
            "paired_host_select_host",
            "paired_host_frontend_inventory",
            "paired_host_frontend_source_inspect",
            "paired_host_frontend_prepare",
            "paired_host_frontend_submit",
            "paired_host_frontend_status"
        ]
    );
    let results: Vec<_> = session
        .messages()
        .iter()
        .filter_map(|message| match message {
            Message::ToolResults { results, .. } => Some(results),
            _ => None,
        })
        .flatten()
        .collect();
    assert_eq!(results.len(), 6);
    assert!(results.iter().all(|result| !result.is_error));
    let status = results
        .iter()
        .find(|result| result.tool_use_id == "role-status")
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&status.text_content()).unwrap(),
        role_built()
    );
    let history = serde_json::to_vec(session.messages()).unwrap();
    for private in [
        CODE,
        DEVICE,
        PAIR,
        TOKEN,
        &host.origin,
        &review_path,
        &approval,
    ] {
        assert!(
            !history
                .windows(private.len())
                .any(|bytes| bytes == private.as_bytes())
        );
    }
    let inputs = timeout(
        BOUND,
        persistence
            .runtime_store()
            .load_input_states_strict(&LogicalRuntimeId::for_session(&session_id)),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(inputs.len(), 2);
    assert_eq!(
        inputs
            .iter()
            .map(|input| input.state.input_id.to_string())
            .collect::<BTreeSet<_>>()
            .len(),
        2
    );
    for input in inputs {
        assert_eq!(input.seed.phase, InputLifecycleState::Consumed);
        assert_eq!(
            input.seed.terminal_outcome,
            Some(InputTerminalOutcome::Consumed)
        );
        assert_eq!(input.seed.attempt_count, 1);
        assert_eq!(input.state.recovery_count, 0);
        let stored = serde_json::to_value(input).unwrap();
        let outcome: CompletionOutcome =
            serde_json::from_value(stored["terminal_completion"]["outcome"].clone()).unwrap();
        assert!(matches!(outcome, CompletionOutcome::Completed(_)));
    }
    println!(
        "CLI role composition: {}",
        json!({"scope":"scripted_model_and_loopback_host",
        "cli_sha256":file_digest(&cli.prefix.join("bin/meerkat-connect")),
        "test_sha256":file_digest(&std::env::current_exe().unwrap()),
        "host_requests":7,"provider_requests":8,"native_sessions":1,"completed_inputs":2,
        "native_history_sha256":format!("{:x}",Sha256::digest(&history)),
        "inspection_sha256":format!("{:x}",Sha256::digest(serde_json::to_vec(&role_inspection()).unwrap())),
        "selected_roles":combined_role_proposal(&role_inspection())["roles"],
        "wrong_approval_submissions":0,"exact_approval_submissions":1,
        "host_attempt":2,"host_state":"built","service_preparations":1,
        "terminal_restored":true,"cli_group_absent":true,"fixture_servers_joined":true})
    );
}

/// Root builds the actual executable and this test with the same resource pin,
/// then supplies a bounded synthetic prefix. No production bypass is installed.
#[tokio::test]
#[ignore = "requires explicitly assembled pinned synthetic Connect resources"]
async fn actual_cli_lazily_prepares_services_then_frontend_in_original_session() {
    let fixture = PathBuf::from(
        std::env::var_os("CONNECT_LAZY_SERVICES_FIXTURE_PREFIX")
            .expect("explicit pinned fixture prefix"),
    );
    assert!(fixture.is_absolute());
    deferred_cli(DeferredMode::SyntheticServices, Some(&fixture)).await;
}

/// Execute the root-owned isolated real prefix without copying or modifying its
/// immutable resource tree. This proves metadata reachability and CLI composition,
/// not service installation, live authentication or frontend activation.
#[tokio::test]
#[ignore = "requires an isolated real supplied library and exact pin-built CLI"]
async fn actual_cli_lazily_prepares_supplied_library_then_frontend() {
    let prefix = PathBuf::from(
        std::env::var_os("CONNECT_REAL_LIBRARY_PREFIX").expect("explicit real supplied prefix"),
    );
    assert!(prefix.is_absolute());
    deferred_cli(DeferredMode::SuppliedLibrary, Some(&prefix)).await;
}

/// Preselecting a host must preserve the same lazy service capability without
/// offering another host selection or requiring a second native conversation.
#[tokio::test]
#[ignore = "requires an isolated real supplied library and exact pin-built CLI"]
async fn actual_cli_prebound_host_retains_supplied_library_in_original_session() {
    let prefix = PathBuf::from(
        std::env::var_os("CONNECT_REAL_LIBRARY_PREFIX").expect("explicit real supplied prefix"),
    );
    assert!(prefix.is_absolute());
    deferred_cli(DeferredMode::PreboundSuppliedLibrary, Some(&prefix)).await;
}

async fn deferred_cli(mode: DeferredMode, fixture: Option<&Path>) {
    let services = mode.services();
    let supplied = mode.supplied();
    let prebound = mode.prebound();
    let mut host = Server::start(false).await;
    let mut provider = if supplied {
        Server::start_mode(true, false, Some(mode)).await
    } else if services {
        Server::start_options(true, false, true).await
    } else {
        Server::start_frontend_only().await
    };
    let mut cli = if supplied {
        Cli::spawn_supplied(
            prebound.then_some(host.origin.as_str()),
            &provider.origin,
            fixture.unwrap(),
        )
    } else {
        Cli::spawn_selected(None, &provider.origin, fixture)
    };
    let evidence_path = supplied
        .then(|| std::env::var_os("CONNECT_REAL_LIBRARY_EVIDENCE"))
        .flatten()
        .map(PathBuf::from);
    let mut evidence = json!({"stage":"spawned","cli_pid":cli.child.id(),
        "state_root":cli.state,"supplied_prefix":cli.prefix,
        "prebound_host":prebound,
        "observation_bound_seconds":cli.observation_bound.as_secs()});
    if supplied {
        let binary = cli.prefix.join("bin/meerkat-connect");
        let executable = std::env::current_exe().unwrap();
        evidence["schema_version"] = json!(1);
        evidence["kind"] = json!("meerkat_connect_supplied_cli_scenario");
        evidence["scenario"] = json!(if prebound {
            "actual_cli_prebound_host_retains_supplied_library_in_original_session"
        } else {
            "actual_cli_lazily_prepares_supplied_library_then_frontend"
        });
        evidence["platform"] = json!({"os":std::env::consts::OS,"arch":std::env::consts::ARCH});
        evidence["cli_artifact"] =
            json!({"sha256":file_digest(&binary),"size":fs::metadata(&binary).unwrap().len()});
        evidence["test_artifact"] = json!({"sha256":file_digest(&executable),"size":fs::metadata(&executable).unwrap().len()});
    }
    if let Some(path) = &evidence_path {
        use std::io::Write;
        assert!(path.is_absolute());
        let mut file = fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(path)
            .unwrap();
        file.write_all(&serde_json::to_vec_pretty(&evidence).unwrap())
            .unwrap();
    }
    cli.shown("New realm: ", 0).await;
    cli.send(&format!("{REALM}\n"));
    cli.approve_setup_helper().await;
    if prebound {
        cli.shown("Type 'request'", 0).await;
        assert!(host.requests.lock().unwrap().is_empty());
        assert_eq!(preparation_output_count(&cli.state), 1);
        pair_fixture_cli(&mut cli).await;
    }
    cli.enter_conversation().await;
    assert_eq!(
        host.requests.lock().unwrap().len(),
        if prebound { 2 } else { 0 }
    );
    assert!(provider.requests.lock().unwrap().is_empty());
    assert_eq!(
        preparation_output_count(&cli.state),
        1,
        "only the supplied setup helper is prepared before the user's first request"
    );
    assert!(
        cli.transcript
            .contains("Preparing the supplied setup helper.")
    );
    let turn_start = cli.transcript.len();
    cli.send(&format!("{}\n", mode.prompt()));
    if !prebound {
        cli.shown("Paired host origin: ", 0).await;
        assert!(host.requests.lock().unwrap().is_empty());
        cli.send(&format!("{}\n", host.origin));
        cli.shown("Type 'remember'", 0).await;
        cli.send("memory\n");
        pair_fixture_cli(&mut cli).await;
    }
    cli.shown("Type 'approve ", turn_start).await;
    assert_eq!(host.submissions(), 0);
    let approval = cli.transcript[turn_start..]
        .split("Type 'approve ")
        .last()
        .unwrap()
        .split('\'')
        .next()
        .unwrap()
        .to_owned();
    cli.send("approve previous-id\n");
    cli.shown("Use the exact approval ID shown above", 0).await;
    assert_eq!(host.submissions(), 0);
    let after = cli.transcript.len();
    cli.send(&format!("approve {approval}\n"));
    cli.shown(ANSWER, after).await;
    cli.shown("You: ", after).await;
    assert_eq!(host.submissions(), 1);
    let followup = cli.transcript.len();
    cli.send(&format!("{FOLLOWUP}\n"));
    cli.shown(FOLLOWUP_ANSWER, followup).await;
    cli.shown("You: ", followup).await;
    cli.send("/quit\n");
    let cli_exit = cli.finish().await;
    assert!(cli_exit.success(), "{}", cli.transcript);
    host.finish().await;
    provider.finish().await;
    assert_eq!(cli.transcript.matches("Setup session: ").count(), 1);
    assert_eq!(
        cli.transcript.matches("Paired host origin: ").count(),
        usize::from(!prebound)
    );
    assert_eq!(
        cli.transcript
            .matches("Enter the six-digit code (hidden)")
            .count(),
        1
    );
    assert_eq!(
        host.requests.lock().unwrap().len(),
        7 + usize::from(prebound),
        "one pairing, four normal first-turn calls, one follow-up inventory and optional early inventory"
    );
    let requests = provider.requests.lock().unwrap();
    let last_messages = requests.last().unwrap().body["messages"]
        .as_array()
        .unwrap();
    let catalog_count = last_messages
        .iter()
        .filter(|message| {
            message["role"] == "tool"
                && message["tool_call_id"]
                    .as_str()
                    .is_some_and(|id| id.starts_with("catalog-"))
        })
        .count();
    assert_eq!(
        requests.len(),
        mode.frontend_start() + 8 - usize::from(prebound) + catalog_count
    );
    if !services {
        assert_eq!(catalog_count, 0);
    }
    assert!(catalog_count <= 12);
    for request in requests.iter() {
        assert!(
            !request.body.to_string().contains(&host.origin),
            "selected origin must stay outside model history"
        );
    }
    let prepared_directory = if supplied {
        let (directory, observation) =
            supplied_library_observation(&cli, &requests.last().unwrap().body, prebound);
        evidence["library"] = observation;
        Some(directory)
    } else {
        None
    };
    let mut paths = vec![cli.state.clone()];
    let mut audit = 0;
    let mut scanned = (0usize, 0u64);
    let mut persisted = (false, false, false, false);
    while let Some(directory) = paths.pop() {
        for entry in fs::read_dir(directory).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                // The genuine bootstrap contains package/runtime trees. Inspect
                // its exact receipts below, not every distributed dependency.
                if prepared_directory.as_ref() != Some(&path) {
                    paths.push(path);
                }
                continue;
            }
            if supplied {
                let metadata = fs::symlink_metadata(&path).unwrap();
                assert!(metadata.is_file() && metadata.len() <= 8 * 1024 * 1024);
                scanned.0 += 1;
                scanned.1 += metadata.len();
                assert!(scanned.0 <= 1024 && scanned.1 <= 32 * 1024 * 1024);
            }
            let bytes = fs::read(&path).unwrap();
            for protected in [CODE, DEVICE, PAIR, TOKEN] {
                assert!(
                    !bytes
                        .windows(protected.len())
                        .any(|part| part == protected.as_bytes()),
                    "protected value persisted in {}",
                    path.display()
                );
            }
            if supplied {
                let text = String::from_utf8_lossy(&bytes);
                persisted.0 |= text.contains(mode.prompt());
                persisted.1 |= text.contains(ANSWER);
                persisted.2 |= text.contains(FOLLOWUP);
                persisted.3 |= text.contains(FOLLOWUP_ANSWER);
            }
            if path.file_name().and_then(|name| name.to_str()) == Some("fixture-mcp.jsonl") {
                audit += 1;
                assert_eq!(
                    String::from_utf8(bytes).unwrap().trim(),
                    r#"{"tool": "setup_help"}"#
                );
            }
        }
    }
    assert_eq!(preparation_output_count(&cli.state), 1);
    assert_eq!(
        audit,
        usize::from(mode == DeferredMode::SyntheticServices),
        "only an activated stdio server may run help, and installation is never dispatched"
    );
    if supplied {
        assert_eq!(
            persisted,
            (true, true, true, true),
            "both turns must remain in native storage"
        );
        assert_eq!(
            cli.transcript
                .matches("Preparing the supplied setup helper.")
                .count(),
            1
        );
        evidence["stage"] = json!("passed");
        evidence["native_session_count"] = json!(1);
        evidence["provider_requests"] = json!(requests.len());
        evidence["native_catalog_control_results"] = json!(catalog_count);
        evidence["frontend_submissions"] = json!(host.submissions());
        evidence["host_requests"] = json!(host.requests.lock().unwrap().len());
        evidence["pairings"] = json!(1);
        evidence["cli_group_absent"] = json!(true);
        // finish() checks both termios and file flags after joining the CLI;
        // both fixture servers above also settle before this passed record.
        evidence["cli_exit_code"] = json!(cli_exit.code());
        evidence["terminal_restored"] = json!(true);
        evidence["fixture_servers_joined"] = json!(true);
        evidence["both_turns_persisted"] = json!(true);
        if let Some(path) = &evidence_path {
            fs::write(path, serde_json::to_vec_pretty(&evidence).unwrap()).unwrap();
        }
    }
}

async fn pair_fixture_cli(cli: &mut Cli) {
    cli.shown("Type 'request'", 0).await;
    assert!(
        cli.transcript
            .contains("OS credential store will not be read or written")
    );
    cli.send("request\n");
    cli.shown("Enter the six-digit code (hidden)", 0).await;
    timeout(BOUND, async {
        while tcgetattr(&cli.observer)
            .unwrap()
            .local_flags
            .contains(LocalFlags::ECHO)
        {
            sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .unwrap();
    cli.send(&format!("{CODE}\n"));
}

fn preparation_output_count(root: &Path) -> usize {
    preparation_artifacts(root).len()
}

fn reviewed_setup_session(
    preparation: &Path,
    setup_approvals: &[String],
    denied_setup_review: &Option<(&'static str, String)>,
) -> meerkat_core::SessionId {
    use meerkat_core::{ApprovalOwnerRef, ApprovalStore};
    // Every live decision owner has its own store, including same-session
    // continuations. Locate the observed startup IDs across all actual stores.
    let mut records = Vec::new();
    for entry in fs::read_dir(preparation).unwrap() {
        let directory = entry.unwrap().path();
        if !directory
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("consent-")
        {
            continue;
        }
        assert!(
            fs::symlink_metadata(&directory).unwrap().is_dir(),
            "a consent owner must be an actual directory"
        );
        let path = directory.join("native-decisions.json");
        assert!(
            fs::symlink_metadata(&path).unwrap().is_file(),
            "the existing native decision store must be an actual file"
        );
        records.extend(
            meerkat_store::FileApprovalStore::open(&path)
                .unwrap()
                .load_all()
                .unwrap(),
        );
    }
    let mut selected = None;
    let expected = setup_approvals
        .iter()
        .enumerate()
        .map(|(index, id)| {
            (
                id,
                meerkat_core::ApprovalStatus::Approved,
                [SETUP_HELPER_REVIEW, SETUP_MODEL_REVIEW][index],
            )
        })
        .chain(
            denied_setup_review
                .iter()
                .map(|(summary, id)| (id, meerkat_core::ApprovalStatus::Denied, *summary)),
        );
    if denied_setup_review.is_some() {
        assert_eq!(
            records.len(),
            setup_approvals.len() + 1,
            "a refused startup cannot silently create another review"
        );
    }
    for (id, status, summary) in expected {
        let matching: Vec<_> = records
            .iter()
            .filter(|record| record.approval_id.to_string() == *id)
            .collect();
        assert_eq!(matching.len(), 1);
        let record = matching[0];
        assert_eq!(record.status, status);
        assert_eq!(record.proposed_action.summary, summary);
        let ApprovalOwnerRef::Session { session_id } = &record.owner else {
            panic!("startup review must belong to the actual selected session")
        };
        if let Some(previous) = &selected {
            assert_eq!(previous, session_id);
        }
        selected = Some(session_id.clone());
    }
    selected.expect("at least one exact startup decision")
}

#[test]
fn reviewed_setup_session_finds_exact_receipts_across_native_stores() {
    use meerkat_core::{
        ApprovalActionKind, ApprovalDecision, ApprovalOwnerRef, ApprovalPrincipalId,
        ApprovalProposedAction, ApprovalRequest, ApprovalResourceId, ApprovalResourceKind,
        ApprovalResourceRef, ApprovalRisk, ApprovalService, SessionId,
    };
    fn decide(
        path: &Path,
        session: &SessionId,
        summary: &str,
        decision: ApprovalDecision,
    ) -> String {
        let service = ApprovalService::with_store(Arc::new(
            meerkat_store::FileApprovalStore::open(path).unwrap(),
        ))
        .unwrap();
        let record = service
            .request(ApprovalRequest {
                requester: ApprovalPrincipalId::new("fixture-requester").unwrap(),
                owner: ApprovalOwnerRef::Session {
                    session_id: session.clone(),
                },
                resource: ApprovalResourceRef {
                    kind: ApprovalResourceKind::Runtime,
                    id: ApprovalResourceId::new("fixture-selected-realm"),
                },
                proposed_action: ApprovalProposedAction {
                    kind: ApprovalActionKind::Other,
                    summary: summary.into(),
                    body: None,
                },
                risk: ApprovalRisk::Medium,
                request_body: None,
                allowed_decisions: [ApprovalDecision::Approve, ApprovalDecision::Deny]
                    .into_iter()
                    .collect(),
                expires_at: None,
                metadata: Default::default(),
                request_provenance: None,
            })
            .unwrap();
        service
            .decide(
                &record.approval_id,
                decision,
                ApprovalPrincipalId::new("fixture-human").unwrap(),
                None,
                None,
            )
            .unwrap();
        record.approval_id.to_string()
    }
    let root = tempfile::tempdir().unwrap();
    let session = meerkat::Session::new().id().clone();
    let first = root.path().join("consent-first/native-decisions.json");
    let second = root
        .path()
        .join("consent-continuation/native-decisions.json");
    let third = root.path().join("consent-empty/native-decisions.json");
    let approvals = vec![
        decide(
            &first,
            &session,
            SETUP_HELPER_REVIEW,
            ApprovalDecision::Approve,
        ),
        decide(
            &first,
            &session,
            SETUP_MODEL_REVIEW,
            ApprovalDecision::Approve,
        ),
    ];
    decide(
        &second,
        &session,
        "A later software review",
        ApprovalDecision::Approve,
    );
    meerkat_store::FileApprovalStore::open(&third).unwrap();
    let empty = fs::read(&third).unwrap();
    assert_eq!(
        reviewed_setup_session(root.path(), &approvals, &None),
        session
    );
    fs::copy(&first, &third).unwrap();
    assert!(
        std::panic::catch_unwind(|| reviewed_setup_session(root.path(), &approvals, &None))
            .is_err(),
        "duplicate observed IDs across owners must not select an arbitrary record"
    );
    fs::write(&third, &empty).unwrap();
    let foreign = meerkat::Session::new().id().clone();
    let foreign_model = decide(
        &third,
        &foreign,
        SETUP_MODEL_REVIEW,
        ApprovalDecision::Approve,
    );
    assert!(
        std::panic::catch_unwind(|| reviewed_setup_session(
            root.path(),
            &[approvals[0].clone(), foreign_model.clone()],
            &None
        ))
        .is_err(),
        "exact startup IDs must still name one common native session"
    );
    fs::write(&third, &empty).unwrap();
    assert!(
        std::panic::catch_unwind(|| reviewed_setup_session(
            root.path(),
            &[approvals[0].clone(), "missing-startup-id".into()],
            &None
        ))
        .is_err()
    );
    let alias = root.path().join("consent-alias");
    std::os::unix::fs::symlink(first.parent().unwrap(), &alias).unwrap();
    let refused =
        std::panic::catch_unwind(|| reviewed_setup_session(root.path(), &approvals, &None));
    fs::remove_file(alias).unwrap();
    assert!(
        refused.is_err(),
        "a consent directory alias must not be followed"
    );
    fs::remove_file(&third).unwrap();
    assert!(
        std::panic::catch_unwind(|| reviewed_setup_session(root.path(), &approvals, &None))
            .is_err()
    );
    assert!(
        !third.exists(),
        "receipt observation must not create a missing store"
    );
    std::os::unix::fs::symlink(&first, &third).unwrap();
    let refused =
        std::panic::catch_unwind(|| reviewed_setup_session(root.path(), &approvals, &None));
    fs::remove_file(&third).unwrap();
    assert!(
        refused.is_err(),
        "the native decision file must not be a symlink"
    );

    let denied_root = tempfile::tempdir().unwrap();
    let initial = denied_root
        .path()
        .join("consent-startup/native-decisions.json");
    let later = denied_root
        .path()
        .join("consent-later/native-decisions.json");
    let approved = vec![decide(
        &initial,
        &session,
        SETUP_HELPER_REVIEW,
        ApprovalDecision::Approve,
    )];
    let denied = Some((
        SETUP_MODEL_REVIEW,
        decide(
            &initial,
            &session,
            SETUP_MODEL_REVIEW,
            ApprovalDecision::Deny,
        ),
    ));
    meerkat_store::FileApprovalStore::open(&later).unwrap();
    assert_eq!(
        reviewed_setup_session(denied_root.path(), &approved, &denied),
        session
    );
    decide(
        &later,
        &session,
        "Unexpected review after refusal",
        ApprovalDecision::Approve,
    );
    assert!(
        std::panic::catch_unwind(|| reviewed_setup_session(denied_root.path(), &approved, &denied))
            .is_err(),
        "a denied startup must count unexpected reviews across every store"
    );
}

fn preparation_artifacts(root: &Path) -> Vec<PathBuf> {
    let parent = root.join(".meerkat-connect").join(REALM);
    if !parent.exists() {
        return Vec::new();
    }
    let directories: Vec<_> = fs::read_dir(parent)
        .unwrap()
        .filter_map(|entry| {
            let path = entry.unwrap().path();
            let metadata = fs::symlink_metadata(&path).unwrap();
            // The realm also owns regular files such as recipes.sqlite and
            // SQLite sidecars. They are not setup invocations.
            if metadata.is_file() {
                return None;
            }
            assert!(
                metadata.is_dir(),
                "setup artifact entries must not be symlinks or special files"
            );
            Some(path)
        })
        .collect();
    assert!(directories.len() <= 16, "bounded setup invocation count");
    directories
        .into_iter()
        .filter(|path| path.join("prepare.stdout").is_file())
        .collect()
}

#[test]
fn preparation_artifacts_allow_realm_files_but_refuse_symlinks_and_excess_directories() {
    let root = tempfile::tempdir().unwrap();
    assert!(preparation_artifacts(root.path()).is_empty());
    let parent = root.path().join(".meerkat-connect").join(REALM);
    fs::create_dir_all(&parent).unwrap();
    for name in ["recipes.sqlite", "recipes.sqlite-wal", "recipes.sqlite-shm"] {
        fs::write(parent.join(name), b"inert realm file; not a preparation").unwrap();
    }
    assert!(preparation_artifacts(root.path()).is_empty());
    let prepared = parent.join("setup-completed");
    fs::create_dir(&prepared).unwrap();
    fs::write(prepared.join("prepare.stdout"), b"inert preparation marker").unwrap();
    fs::create_dir(parent.join("setup-pending")).unwrap();
    assert_eq!(preparation_artifacts(root.path()), vec![prepared.clone()]);
    for target in [&prepared, &parent.join("recipes.sqlite")] {
        let alias = parent.join("setup-alias");
        std::os::unix::fs::symlink(target, &alias).unwrap();
        let refused = std::panic::catch_unwind(|| preparation_artifacts(root.path()));
        fs::remove_file(alias).unwrap();
        assert!(
            refused.is_err(),
            "the observer must not follow realm symlinks"
        );
    }
    for index in 2..16 {
        fs::create_dir(parent.join(format!("setup-pending-{index}"))).unwrap();
    }
    assert_eq!(
        preparation_artifacts(root.path()),
        vec![prepared],
        "regular realm files do not consume the setup-directory bound"
    );
    fs::create_dir(parent.join("setup-seventeenth")).unwrap();
    assert!(
        std::panic::catch_unwind(|| preparation_artifacts(root.path())).is_err(),
        "the setup invocation bound still applies to real directories"
    );
}

fn read_bounded_json(path: &Path, limit: u64) -> (Vec<u8>, Value) {
    let metadata = fs::symlink_metadata(path).unwrap();
    assert!(metadata.is_file() && metadata.len() <= limit);
    let raw = fs::read(path).unwrap();
    assert!(raw.len() as u64 <= limit);
    let value = serde_json::from_slice(&raw).unwrap();
    (raw, value)
}

fn file_digest(path: &Path) -> String {
    use std::io::Read;
    let mut file = fs::File::open(path).unwrap();
    let mut digest = Sha256::new();
    let mut bytes = [0; 64 * 1024];
    loop {
        let count = file.read(&mut bytes).unwrap();
        if count == 0 {
            break;
        }
        digest.update(&bytes[..count]);
    }
    format!("{:x}", digest.finalize())
}

fn supplied_library_observation(
    cli: &Cli,
    last_request: &Value,
    prebound: bool,
) -> (PathBuf, Value) {
    let artifacts = preparation_artifacts(&cli.state);
    assert_eq!(
        artifacts.len(),
        1,
        "one real verified bootstrap, never a silent retry"
    );
    let prepared = artifacts[0].join("prepared");
    let (_, handoff) = read_bounded_json(&artifacts[0].join("prepare.stdout"), 256 * 1024);
    let (config_raw, config) = read_bounded_json(&prepared.join("config/mcp.json"), 256 * 1024);
    assert_eq!(
        handoff, config,
        "native receives the verified owner's exact command"
    );
    let server = &config["mcpServers"]["toolkit-setup"];
    assert_eq!(config["mcpServers"].as_object().unwrap().len(), 1);
    assert_eq!(server["command"], "/usr/bin/env");
    let args: Vec<_> = server["args"]
        .as_array()
        .unwrap()
        .iter()
        .map(|arg| arg.as_str().unwrap())
        .collect();
    assert_eq!(args.first(), Some(&"-i"));
    assert!(
        !args.iter().any(|arg| arg.starts_with("--browser")),
        "ordinary stdio setup starts no browser companion"
    );
    let workspace_index = args.iter().position(|arg| *arg == "--workspace").unwrap();
    assert_eq!(
        fs::canonicalize(args[workspace_index + 1]).unwrap(),
        fs::canonicalize(prepared.join("session")).unwrap()
    );

    let (_, product) = read_bounded_json(&prepared.join("product-prepared.json"), 4 * 1024 * 1024);
    let resource_pin = file_digest(&cli.prefix.join("libexec/meerkat-connect/resources.json"));
    assert_eq!(product["resources_sha256"], resource_pin);
    assert_eq!(
        product["mcp_sha256"],
        format!("{:x}", Sha256::digest(&config_raw))
    );
    let (_, receipt) = read_bounded_json(&prepared.join("config/prepared.json"), 4 * 1024 * 1024);
    assert_eq!(receipt["status"], "prepared");
    assert_eq!(receipt["scope"], "host_setup_configuration_only");

    let catalog = tool_result(last_request, "library-catalog");
    let entry = tool_result(last_request, "library-entry");
    let state = tool_result(last_request, "library-state");
    assert_supplied_catalog(&catalog);
    assert_idle_library(&state);
    let workspace = prepared.join("session");
    let (catalog_raw, frozen_catalog) =
        read_bounded_json(&workspace.join("bundle/catalog.json"), 2 * 1024 * 1024);
    assert_eq!(
        catalog["catalog_sha256"],
        format!("{:x}", Sha256::digest(&catalog_raw))
    );
    assert_eq!(
        catalog["contents_sha256"],
        file_digest(&workspace.join("bundle/CONTENTS.json"))
    );
    let rows = frozen_catalog.as_array().unwrap();
    let summaries: Vec<Value> = rows.iter().map(|row| json!({
        "identity":row["identity"],"display_name":row["display_name"],"description":row["description"]
    })).collect();
    assert_eq!(
        catalog["entries"],
        json!(summaries),
        "real metadata preserves the pinned catalog's records and order"
    );
    assert_eq!(
        entry["entry"], rows[0],
        "the complete selected record must survive the real MCP/native path"
    );
    for name in ["inspection", "configurations", "installations", "requests"] {
        assert_eq!(
            fs::read_dir(workspace.join(name)).unwrap().count(),
            0,
            "no setup job may populate {name}"
        );
    }
    let calls: Vec<_> = last_request["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|message| message["role"] == "assistant")
        .flat_map(|message| message["tool_calls"].as_array().into_iter().flatten())
        .filter_map(|call| call["function"]["name"].as_str())
        .filter(|name| !matches!(*name, "tool_catalog_search" | "tool_catalog_load"))
        .collect();
    let mut expected = Vec::new();
    if prebound {
        expected.push("paired_host_frontend_inventory");
    }
    expected.extend([
        "toolkit_frontend_catalog",
        "toolkit_prepare_services",
        "setup_catalog",
        "setup_catalog_entry",
        "setup_install",
        "setup_state",
    ]);
    if !prebound {
        expected.push("paired_host_select_host");
    }
    expected.extend([
        "paired_host_frontend_inventory",
        "paired_host_frontend_prepare",
        "paired_host_frontend_submit",
        "paired_host_frontend_status",
    ]);
    if !prebound {
        expected.push("paired_host_select_host");
    }
    expected.push("paired_host_frontend_inventory");
    assert_eq!(calls, expected);
    (
        prepared,
        json!({
            "resources_sha256":resource_pin,"catalog_sha256":catalog["catalog_sha256"],
            "contents_sha256":catalog["contents_sha256"],"distinct_catalog_records":rows.len(),
            "catalog_scope":catalog["scope"],"complete_entry_matches_pin":true,
            "preparation_count":artifacts.len(),"job_count":state["job_history"]["count"],
            "all_families_unprepared":true,"request_and_install_directories_empty":true,
            "browser_companion_requested":false,"native_install_denied":true
        }),
    )
}

#[path = "fixtures/forge-auth/runner.rs"]
mod forge_owner;

/// Real copied executable/native session and frozen Forge auth, scripted model.
/// No deployed app, producer configuration, activation or live provider account.
#[tokio::test]
#[ignore = "requires CONNECT_FORGE_TEST_PYTHON with the existing Forge auth environment"]
async fn actual_cli_forge_sign_in_reports_unsupported_without_paired_host_or_replay() {
    use meerkat_core::types::Message;
    use meerkat_runtime::{
        completion::CompletionOutcome,
        identifiers::LogicalRuntimeId,
        input_state::{InputLifecycleState, InputTerminalOutcome},
    };
    const FIRST: &str = "Check whether my local Forge application supports agent frontends. Let me select it and sign in privately.";
    const ANSWER: &str = "The selected Forge application reports frontend management unsupported. No frontend host identity, configuration or activation was established.";
    const SECOND: &str =
        "Keep this same conversation and summarize that observation. Do not sign in again.";
    const FINAL: &str = "The earlier observation remains unsupported. No further login, service preparation or frontend action was attempted.";
    let mut owner = forge_owner::Owner::start().await;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let (stop, mut stopped) = oneshot::channel();
    let requests = Arc::new(Mutex::new(Vec::<Request>::new()));
    let observed = requests.clone();
    let private_origin = owner.origin.clone();
    let task = tokio::spawn(async move {
        loop {
            let mut stream = tokio::select! {
                biased;
                _ = &mut stopped => break,
                accepted = listener.accept() => accepted.unwrap().0,
            };
            let received = timeout(BOUND, request(&mut stream)).await.unwrap();
            assert_eq!(received.method, "POST");
            assert_eq!(received.path, "/v1/chat/completions");
            assert_eq!(received.headers["authorization"], format!("Bearer {TOKEN}"));
            assert!(!received.headers.contains_key("cookie"));
            assert_eq!(received.body["model"], "cli-fixture-remote");
            let serialized = received.body.to_string();
            for private in [
                forge_owner::PASSWORD,
                private_origin.as_str(),
                TOKEN,
                DEVICE,
                PAIR,
                CODE,
            ] {
                assert!(
                    !serialized.contains(private),
                    "protected material entered model context"
                );
            }
            let phase = observed.lock().unwrap().len();
            let names: Vec<_> = received.body["tools"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(|tool| tool["function"]["name"].as_str())
                .collect();
            assert!(names.contains(&"forge_select_host"));
            assert!(!names.contains(&"setup_catalog") && !names.contains(&"setup_install"));
            assert!(names.contains(&"forge_frontend_inspect"));
            let messages = received.body["messages"].as_array().unwrap();
            let users: Vec<_> = messages
                .iter()
                .filter(|row| row["role"] == "user")
                .filter_map(|row| row["content"].as_str())
                .filter(|text| [FIRST, SECOND].contains(text))
                .collect();
            assert_eq!(
                users,
                if phase < 2 {
                    vec![FIRST]
                } else {
                    vec![FIRST, SECOND]
                }
            );
            let body = if phase == 0 {
                deferred_call_response("forge-check", "forge_select_host", json!({}))
            } else {
                assert_eq!(
                    tool_result(&received.body, "forge-check"),
                    json!({"frontend_management":"unsupported"})
                );
                assert!(phase <= 2, "no provider retry or extra turn");
                if phase == 2 {
                    assert!(
                        messages
                            .iter()
                            .any(|row| row["role"] == "assistant" && row["content"] == ANSWER)
                    );
                }
                let event = json!({"choices":[{"delta":{"content":if phase == 1 {ANSWER} else {FINAL}},"finish_reason":"stop"}]});
                format!(
                    "data: {event}\n\ndata: {{\"choices\":[],\"usage\":{{\"prompt_tokens\":100,\"completion_tokens\":20,\"total_tokens\":120}}}}\n\ndata: [DONE]\n\n"
                )
            };
            observed.lock().unwrap().push(received);
            let response = format!(
                "HTTP/1.1 200 Fixture\r\nConnection: close\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            );
            timeout(BOUND, async {
                stream.write_all(response.as_bytes()).await.unwrap();
                stream.shutdown().await.unwrap();
            })
            .await
            .unwrap();
        }
    });
    let mut provider = Server {
        origin,
        requests,
        stop: Some(stop),
        task: Some(task),
    };
    let mut cli = Cli::spawn_selected(None, &provider.origin, None);
    let config = fs::read(cli.state.join(REALM).join("config.toml")).unwrap();
    cli.shown("New realm: ", 0).await;
    cli.send(&format!("{REALM}\n"));
    cli.approve_setup_helper().await;
    cli.enter_conversation().await;
    assert!(provider.requests.lock().unwrap().is_empty());
    cli.send(&format!("{FIRST}\n"));
    cli.shown("Forge Console origin", 0).await;
    cli.send(&format!("{}\n", owner.origin));
    cli.shown("Forge Console password (hidden)", 0).await;
    assert!(
        !tcgetattr(&cli.observer)
            .unwrap()
            .local_flags
            .intersects(LocalFlags::ECHO | LocalFlags::ICANON)
    );
    cli.send(&format!("{}\n", forge_owner::PASSWORD));
    cli.shown(ANSWER, 0).await;
    cli.shown("You: ", cli.transcript.find(ANSWER).unwrap())
        .await;
    assert_eq!(tcgetattr(&cli.observer).unwrap(), cli.termios);
    let after = cli.transcript.len();
    cli.send(&format!("{SECOND}\n"));
    cli.shown(FINAL, after).await;
    cli.shown("You: ", after).await;
    cli.send("/quit\n");
    assert!(cli.finish().await.success(), "{}", cli.transcript);
    provider.finish().await;
    let canaries = owner.finish().await;
    assert_eq!(cli.transcript.matches("Setup session: ").count(), 1);
    assert_eq!(cli.transcript.matches("Forge Console origin").count(), 1);
    assert_eq!(
        cli.transcript
            .matches("Forge Console password (hidden)")
            .count(),
        1
    );
    assert!(
        !cli.transcript.contains("Paired host origin:")
            && !cli.transcript.contains("Type 'request'")
    );
    assert_eq!(preparation_output_count(&cli.state), 1);
    cli.assert_reviewed_setup(&config);
    let private: Vec<_> = [forge_owner::PASSWORD, owner.origin.as_str()]
        .into_iter()
        .chain(canaries.iter().map(String::as_str))
        .collect();
    for value in canaries
        .iter()
        .map(String::as_str)
        .chain([forge_owner::PASSWORD])
    {
        assert!(!cli.transcript.contains(value));
    }
    {
        let requests = provider.requests.lock().unwrap();
        assert_eq!(requests.len(), 3);
        for request in requests.iter() {
            for value in &private {
                assert!(!request.body.to_string().contains(value));
            }
        }
    }
    let session_id = meerkat_core::SessionId::parse(
        cli.transcript
            .split("Setup session: ")
            .nth(1)
            .unwrap()
            .split_whitespace()
            .next()
            .unwrap(),
    )
    .unwrap();
    let root = cli._root.as_ref().unwrap().path();
    let storage = meerkat_connect::RealmOptions {
        context_root: root.join("outside-checkout"),
        state_root: Some(cli.state.clone()),
        user_config_root: Some(root.join("home")),
    }
    .resolve(REALM)
    .unwrap();
    let (_, persistence) = timeout(
        BOUND,
        meerkat::governed_realm::open_stock_realm_persistence_as(
            &meerkat::DiskStorageProvider,
            &storage.locator.state_root,
            REALM,
            None,
            Some(meerkat_store::RealmOrigin::Explicit),
            Some(storage.layout),
            meerkat::governed_realm::stock::issuance::DEFAULT_STOCK_AGENT,
        ),
    )
    .await
    .unwrap()
    .unwrap();
    let session = timeout(BOUND, persistence.session_store().load(&session_id))
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let conversation: Vec<_> = session
        .messages()
        .iter()
        .filter_map(|message| match message {
            Message::User(user) => Some(("user", user.text_content())),
            Message::BlockAssistant(assistant) if !assistant.to_string().is_empty() => {
                Some(("assistant", assistant.to_string()))
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        conversation,
        [
            ("user", FIRST.into()),
            ("assistant", ANSWER.into()),
            ("user", SECOND.into()),
            ("assistant", FINAL.into())
        ]
    );
    let results: Vec<_> = session
        .messages()
        .iter()
        .filter_map(|message| match message {
            Message::ToolResults { results, .. } => Some(results),
            _ => None,
        })
        .flatten()
        .collect();
    assert_eq!(results.len(), 1);
    assert!(!results[0].is_error);
    assert_eq!(
        serde_json::from_str::<Value>(&results[0].text_content()).unwrap(),
        json!({"frontend_management":"unsupported"})
    );
    let inputs = timeout(
        BOUND,
        persistence
            .runtime_store()
            .load_input_states_strict(&LogicalRuntimeId::for_session(&session_id)),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(inputs.len(), 2);
    for input in inputs {
        assert_eq!(input.seed.phase, InputLifecycleState::Consumed);
        assert_eq!(
            input.seed.terminal_outcome,
            Some(InputTerminalOutcome::Consumed)
        );
        assert_eq!(input.seed.attempt_count, 1);
        assert_eq!(input.state.recovery_count, 0);
        let stored = serde_json::to_value(input).unwrap();
        let outcome: CompletionOutcome =
            serde_json::from_value(stored["terminal_completion"]["outcome"].clone()).unwrap();
        assert!(matches!(outcome, CompletionOutcome::Completed(_)));
    }
    let history = serde_json::to_vec(session.messages()).unwrap();
    for value in &private {
        assert!(
            !history
                .windows(value.len())
                .any(|bytes| bytes == value.as_bytes())
        );
    }
    let mut paths = vec![cli.state.clone()];
    while let Some(directory) = paths.pop() {
        for entry in fs::read_dir(directory).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                paths.push(path);
                continue;
            }
            assert!(fs::metadata(&path).unwrap().len() <= 16 * 1024 * 1024);
            let bytes = fs::read(path).unwrap();
            for value in &private {
                assert!(
                    !bytes
                        .windows(value.len())
                        .any(|part| part == value.as_bytes())
                );
            }
        }
    }
    println!(
        "CLI Forge composition: {}",
        json!({"scope":"scripted_model_frozen_auth_loopback",
        "cli_sha256":file_digest(&cli.prefix.join("bin/meerkat-connect")),
        "test_sha256":file_digest(&std::env::current_exe().unwrap()),
        "native_history_sha256":format!("{:x}",Sha256::digest(history)),
        "native_sessions":1,"completed_inputs":2,"provider_requests":3,
        "login_posts":1,"support_gets":1,"support_status":501,"host_identity":null,
        "service_preparations":1,"terminal_restored":true,"cli_group_absent":true,"owner_group_absent":true})
    );
}

#[path = "fixtures/forge-reading/runner.rs"]
mod forge_reading_owner;

#[path = "cli_paired_host/reading_management.rs"]
mod reading_management;

/// Two separate copied executables share only the ordinary selected realm.
/// The scripted model consumes real owner results; no recovery JSON is injected.
#[tokio::test]
#[ignore = "requires CONNECT_FORGE_TEST_PYTHON with the pinned Forge environment"]
async fn actual_cli_forge_configuration_recovers_after_process_restart_without_replay() {
    use meerkat_core::types::Message;
    use meerkat_runtime::{
        completion::CompletionOutcome,
        identifiers::LogicalRuntimeId,
        input_state::{InputLifecycleState, InputTerminalOutcome},
    };
    const FIRST: &str = "Inspect the frontend choices on my local Forge Reading application. Ask me for the required explicit choices before preparing.";
    const QUESTION: &str = "Reading offers its owner participant and lead DM route. Supply your numeric Telegram human ID and confirm exclusive polling before configuration. This does not provision a token or verify identity.";
    const CHOICE: &str = "Use the advertised owner participant and DM route for my Telegram human ID 123456789. I acknowledge exclusive polling. Prepare the exact configuration for private approval.";
    const UNKNOWN: &str = "The submitted configuration response is uncertain. The exact change is retained for status recovery; do not submit it again. Activation and delivery remain unverified.";
    const RECOVER: &str = "Recover my previous Reading frontend change from this realm. Let me sign in privately and read only its exact status. Do not prepare or submit anything.";
    const RECOVERED: &str = "The owner reports the retained configuration committed at revision 1. This status read did not repeat submission. The current activation observation does not establish delivery or token provisioning.";
    let mut owner = forge_reading_owner::Owner::start().await;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let (stop, mut stopped) = oneshot::channel();
    let requests = Arc::new(Mutex::new(Vec::<Request>::new()));
    let observed = requests.clone();
    let private_origin = owner.origin.clone();
    let task = tokio::spawn(async move {
        loop {
            let mut stream = tokio::select! {biased; _=&mut stopped=>break, accepted=listener.accept()=>accepted.unwrap().0};
            let received = timeout(BOUND, request(&mut stream)).await.unwrap();
            assert_eq!(received.method, "POST");
            assert_eq!(received.path, "/v1/chat/completions");
            assert_eq!(received.headers["authorization"], format!("Bearer {TOKEN}"));
            assert_eq!(received.body["model"], "cli-fixture-remote");
            assert!(!received.headers.contains_key("cookie"));
            for secret in [
                forge_reading_owner::PASSWORD,
                private_origin.as_str(),
                TOKEN,
                DEVICE,
                PAIR,
                CODE,
            ] {
                assert!(
                    !received.body.to_string().contains(secret),
                    "protected value entered model context"
                );
            }
            let phase = observed.lock().unwrap().len();
            let names: Vec<_> = received.body["tools"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(|t| t["function"]["name"].as_str())
                .collect();
            assert!(names.contains(&"forge_select_host"));
            assert!(!names.contains(&"setup_install"));
            let users: Vec<_> = received.body["messages"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|m| m["role"] == "user")
                .filter_map(|m| m["content"].as_str())
                .filter(|s| [FIRST, CHOICE, RECOVER].contains(s))
                .collect();
            assert_eq!(
                users,
                if phase < 3 {
                    vec![FIRST]
                } else if phase < 6 {
                    vec![FIRST, CHOICE]
                } else {
                    vec![RECOVER]
                }
            );
            let call = match phase {
                0 => Some(("reading-host", "forge_select_host", json!({}))),
                1 => {
                    assert_eq!(
                        tool_result(&received.body, "reading-host")["frontend_management"],
                        "declared"
                    );
                    Some(("reading-inventory", "forge_frontend_inspect", json!({})))
                }
                2 => {
                    let inventory = tool_result(&received.body, "reading-inventory");
                    assert_eq!(inventory["observation"]["revision"], 0);
                    None
                }
                3 => {
                    let inventory = tool_result(&received.body, "reading-inventory");
                    let source = &inventory["observation"];
                    Some((
                        "reading-prepare",
                        "forge_frontend_prepare",
                        json!({"participant_id":source["participants"][0]["id"],"route_id":source["routes"][0]["id"],
                        "telegram_user_id":123456789,"exclusive_polling_acknowledged":true,"expected_revision":source["revision"]}),
                    ))
                }
                4 => {
                    let prepared = tool_result(&received.body, "reading-prepare");
                    assert_eq!(prepared["approval_required"], true);
                    Some((
                        "reading-submit",
                        "forge_frontend_submit",
                        json!({"change_id":prepared["proposal"]["change_id"]}),
                    ))
                }
                5 => {
                    let result = received.body["messages"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .find(|m| m["role"] == "tool" && m["tool_call_id"] == "reading-submit")
                        .unwrap();
                    assert!(
                        result["content"]
                            .as_str()
                            .unwrap()
                            .contains("could not be confirmed")
                    );
                    None
                }
                6 => Some(("recovered-host", "forge_select_host", json!({}))),
                7 => {
                    // The fresh agent receives this locator only after protected
                    // discovery/sign-in. No fixture-known ID is supplied to it.
                    let selected = tool_result(&received.body, "recovered-host");
                    assert_eq!(selected["recovery"]["status_only"], true);
                    Some((
                        "recovered-status",
                        "forge_frontend_status",
                        json!({"change_id":selected["recovery"]["change_id"]}),
                    ))
                }
                8 => {
                    let status = tool_result(&received.body, "recovered-status");
                    assert_eq!(status["observation"]["status"], "committed");
                    assert_eq!(status["observation"]["result"]["revision"], 1);
                    assert_eq!(status["recovery_only"], true);
                    None
                }
                _ => panic!("unexpected model call or replay"),
            };
            let body = if let Some((id, name, args)) = call {
                assert!(names.contains(&name));
                deferred_call_response(id, name, args)
            } else {
                let answer = match phase {
                    2 => QUESTION,
                    5 => UNKNOWN,
                    8 => RECOVERED,
                    _ => unreachable!(),
                };
                let event =
                    json!({"choices":[{"delta":{"content":answer},"finish_reason":"stop"}]});
                format!(
                    "data: {event}\n\ndata: {{\"choices\":[],\"usage\":{{\"prompt_tokens\":100,\"completion_tokens\":20,\"total_tokens\":120}}}}\n\ndata: [DONE]\n\n"
                )
            };
            observed.lock().unwrap().push(received);
            let response = format!(
                "HTTP/1.1 200 Fixture\r\nConnection: close\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            );
            timeout(BOUND, async {
                stream.write_all(response.as_bytes()).await.unwrap();
                stream.shutdown().await.unwrap();
            })
            .await
            .unwrap();
        }
    });
    let mut provider = Server {
        origin,
        requests,
        stop: Some(stop),
        task: Some(task),
    };
    let mut first = Cli::spawn_selected(None, &provider.origin, None);
    let config = fs::read(first.state.join(REALM).join("config.toml")).unwrap();
    first.shown("New realm: ", 0).await;
    first.send(&format!("{REALM}\n"));
    first.approve_setup_helper().await;
    first.enter_conversation().await;
    first.send(&format!("{FIRST}\n"));
    first.shown("Forge Console origin", 0).await;
    first.send(&format!("{}\n", owner.origin));
    first.shown("Forge Console password (hidden)", 0).await;
    assert!(
        !tcgetattr(&first.observer)
            .unwrap()
            .local_flags
            .intersects(LocalFlags::ECHO | LocalFlags::ICANON)
    );
    first.send(&format!("{}\n", forge_reading_owner::PASSWORD));
    first.shown(QUESTION, 0).await;
    let after = first.transcript.find(QUESTION).unwrap();
    first.shown("You: ", after).await;
    owner.control("fail-next-submit").await;
    first.send(&format!("{CHOICE}\n"));
    first.shown("Type 'approve ", after).await;
    let approval = first.transcript[after..]
        .split("Type 'approve ")
        .nth(1)
        .unwrap()
        .split('\'')
        .next()
        .unwrap()
        .to_owned();
    assert!(first.transcript.contains("messages already queued"));
    first.send("approve wrong-id\n");
    first.shown("Use the exact approval ID", after).await;
    assert!(
        !owner.control("observe").await["commands"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["operation"] == "submit")
    );
    first.send(&format!("approve {approval}\n"));
    first.shown(UNKNOWN, after).await;
    first
        .shown("You: ", first.transcript.find(UNKNOWN).unwrap())
        .await;
    first.send("/quit\n");
    assert!(
        !first.finish().await.success(),
        "uncertain effect remains an honest nonzero result"
    );
    assert!(first.transcript.contains("Its approval is spent"));
    let artifacts = first.state.join(".meerkat-connect").join(REALM);
    let references: Vec<_> = fs::read_dir(&artifacts)
        .unwrap()
        .flat_map(|p| fs::read_dir(p.unwrap().path()).unwrap())
        .map(|p| p.unwrap().path())
        .filter(|p| {
            p.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("forge-change-")
        })
        .map(|p| {
            let bytes = fs::read(&p).unwrap();
            (p, bytes)
        })
        .collect();
    assert_eq!(references.len(), 1);
    let retained: Value = serde_json::from_slice(&references[0].1).unwrap();
    assert!(!retained.as_object().unwrap().contains_key("approval"));
    owner.control("expire").await;
    first.assert_reviewed_setup(&config);
    let first_pid = first.child.id();
    let root = first._root.take().unwrap();
    let mut second =
        Cli::spawn_configured_in(root, None, &provider.origin, None, false, None, None);
    assert_ne!(first_pid, second.child.id());
    second
        .shown("Choose a realm number, 'new', or /quit: ", 0)
        .await;
    assert!(second.transcript.contains(&format!("1. {REALM} (")));
    second.send("1\n");
    second.approve_setup_helper().await;
    second.enter_conversation().await;
    second.send(&format!("{RECOVER}\n"));
    second.shown("Previous Forge changes", 0).await;
    assert!(
        second
            .transcript
            .contains(retained["change_id"].as_str().unwrap())
    );
    second.send("1\n");
    second.shown("Forge Console password (hidden)", 0).await;
    assert!(!second.transcript.contains("Forge Console origin ("));
    second.send(&format!("{}\n", forge_reading_owner::PASSWORD));
    second.shown(RECOVERED, 0).await;
    second
        .shown("You: ", second.transcript.find(RECOVERED).unwrap())
        .await;
    second.send("/quit\n");
    assert!(second.finish().await.success());
    provider.finish().await;
    let (result, canaries) = owner.finish().await;
    assert_eq!(
        result["commands"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|r| r["operation"] == "submit")
            .count(),
        1
    );
    assert_eq!(
        result["commands"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|r| r["operation"] == "prepare")
            .count(),
        1
    );
    for path in [
        "/console/forge/frontends/prepare",
        "/console/forge/frontends/submit",
    ] {
        assert_eq!(
            result["requests"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|request| request["method"] == "POST" && request["path"] == path)
                .count(),
            1
        );
    }
    assert_eq!(result["state"]["revision"], 1);
    assert_eq!(fs::read(&references[0].0).unwrap(), references[0].1);
    assert_eq!(preparation_output_count(&second.state), 2);
    second.assert_reviewed_setup(&second.initial_config);
    assert_eq!(second.transcript.matches("Type 'approve ").count(), 2);
    {
        let calls = provider.requests.lock().unwrap();
        assert_eq!(calls.len(), 9);
        let prepared = tool_result(&calls[4].body, "reading-prepare");
        let recovered = tool_result(&calls[8].body, "recovered-status");
        assert_eq!(prepared["proposal"], recovered["observation"]["proposal"]);
        for call in calls.iter() {
            for secret in canaries.iter().map(String::as_str).chain([
                forge_reading_owner::PASSWORD,
                owner.origin.as_str(),
                approval.as_str(),
            ]) {
                assert!(!call.body.to_string().contains(secret));
            }
        }
    }
    let root = second._root.as_ref().unwrap().path();
    let storage = meerkat_connect::RealmOptions {
        context_root: root.join("outside-checkout"),
        state_root: Some(second.state.clone()),
        user_config_root: Some(root.join("home")),
    }
    .resolve(REALM)
    .unwrap();
    let (_, persistence) = timeout(
        BOUND,
        meerkat::governed_realm::open_stock_realm_persistence_as(
            &meerkat::DiskStorageProvider,
            &storage.locator.state_root,
            REALM,
            None,
            Some(meerkat_store::RealmOrigin::Explicit),
            Some(storage.layout),
            meerkat::governed_realm::stock::issuance::DEFAULT_STOCK_AGENT,
        ),
    )
    .await
    .unwrap()
    .unwrap();
    let mut sessions = Vec::new();
    for (cli, users, answers) in [
        (&first, vec![FIRST, CHOICE], vec![QUESTION, UNKNOWN]),
        (&second, vec![RECOVER], vec![RECOVERED]),
    ] {
        assert_eq!(cli.transcript.matches("Setup session: ").count(), 1);
        let id = meerkat_core::SessionId::parse(
            cli.transcript
                .split("Setup session: ")
                .nth(1)
                .unwrap()
                .split_whitespace()
                .next()
                .unwrap(),
        )
        .unwrap();
        sessions.push(id.clone());
        let stored = persistence
            .session_store()
            .load(&id)
            .await
            .unwrap()
            .unwrap();
        let actual_users: Vec<_> = stored
            .messages()
            .iter()
            .filter_map(|m| match m {
                Message::User(u) => Some(u.text_content()),
                _ => None,
            })
            .collect();
        assert_eq!(actual_users, users);
        let actual_answers: Vec<_> = stored
            .messages()
            .iter()
            .filter_map(|m| match m {
                Message::BlockAssistant(a) if !a.to_string().is_empty() => Some(a.to_string()),
                _ => None,
            })
            .collect();
        assert_eq!(actual_answers, answers);
        let inputs = persistence
            .runtime_store()
            .load_input_states_strict(&LogicalRuntimeId::for_session(&id))
            .await
            .unwrap();
        assert_eq!(inputs.len(), users.len());
        for input in inputs {
            assert_eq!(input.seed.phase, InputLifecycleState::Consumed);
            assert_eq!(
                input.seed.terminal_outcome,
                Some(InputTerminalOutcome::Consumed)
            );
            assert_eq!(input.seed.attempt_count, 1);
            assert_eq!(input.state.recovery_count, 0);
            let value = serde_json::to_value(input).unwrap();
            let outcome: CompletionOutcome =
                serde_json::from_value(value["terminal_completion"]["outcome"].clone()).unwrap();
            assert!(matches!(outcome, CompletionOutcome::Completed(_)));
        }
        let history = serde_json::to_string(stored.messages()).unwrap();
        for secret in canaries.iter().map(String::as_str).chain([
            forge_reading_owner::PASSWORD,
            owner.origin.as_str(),
            approval.as_str(),
        ]) {
            assert!(!history.contains(secret));
            assert!(
                !cli.transcript.contains(secret) || secret == owner.origin || secret == approval
            );
        }
    }
    assert_ne!(sessions[0], sessions[1]);
    println!(
        "CLI Forge recovery: {}",
        json!({"scope":"two_copied_cli_processes_frozen_owner_scripted_model","cli_sha256":file_digest(&second.prefix.join("bin/meerkat-connect")),"test_sha256":file_digest(&std::env::current_exe().unwrap()),"processes":2,"native_sessions":2,"completed_inputs":3,"prepare_posts":1,"submit_posts":1,"configuration_revision":1,"manual_recovery_files":0,"service_preparations":2,"provider_activation":false,"all_owned_processes_joined":true})
    );
}

/// Private JSONL peer for the actual copied executable. No terminal simulation,
/// browser auth, native override, injected dispatcher or second runtime.
struct StdioCli {
    root: tempfile::TempDir,
    child: Option<Child>,
    stdout: std::process::ChildStdout,
    rows: Vec<Value>,
    binary: PathBuf,
    state: PathBuf,
    context: PathBuf,
    home: PathBuf,
}
impl StdioCli {
    fn start(provider: &str) -> Self {
        const PIN: &str = "61cf296a93b5bda5ab723ef1e7c7b81e0767d074a43690ab534b8a371ac41ca8";
        assert_eq!(option_env!("MEERKAT_CONNECT_RESOURCES_SHA256"), Some(PIN));
        let root = tempfile::tempdir().unwrap();
        let state = root.path().join("state");
        let context = root.path().join("outside-checkout");
        let home = root.path().join("home");
        for path in [&state.join(REALM), &context, &home] {
            fs::create_dir_all(path).unwrap();
        }
        let prefix = PathBuf::from(
            std::env::var("CONNECT_STDIO_CANDIDATE_PREFIX")
                .expect("explicit fresh candidate output"),
        );
        assert!(prefix.is_absolute() && !prefix.exists());
        fs::create_dir_all(prefix.join("bin")).unwrap();
        let binary = prefix.join("bin/meerkat-connect");
        fs::copy(env!("CARGO_BIN_EXE_meerkat-connect"), &binary).unwrap();
        assert_eq!(
            file_digest(&binary),
            file_digest(Path::new(env!("CARGO_BIN_EXE_meerkat-connect")))
        );
        let config = format!(
            r#"
[agent]
model = "cli-fixture"
max_turns = 12
[self_hosted.servers.cli]
transport = "openai_compatible"
api_style = "chat_completions"
base_url = "{provider}/v1"
[self_hosted.models.cli-fixture]
server = "cli"
remote_model = "cli-fixture-remote"
display_name = "CLI loopback fixture"
family = "cli-fixture"
context_window = 32768
max_output_tokens = 1024
call_timeout_secs = 10
[realm.cli-frontend]
default_binding = "fixture"
[realm.cli-frontend.backend.fixture]
provider = "self_hosted"
backend_kind = "self_hosted"
server = "cli"
base_url = "{provider}/v1"
[realm.cli-frontend.auth.fixture]
provider = "self_hosted"
auth_method = "static_bearer"
source = {{kind = "env", env = "CONNECT_FIXTURE_BEARER"}}
[realm.cli-frontend.binding.fixture]
backend_profile = "fixture"
auth_profile = "fixture"
default_model = "cli-fixture"
"#
        );
        meerkat_core::Config::from_persisted_toml(&config)
            .unwrap()
            .0
            .validate(meerkat_models::canonical())
            .unwrap();
        fs::write(state.join(REALM).join("config.toml"), config).unwrap();
        let mut child = Command::new(&binary)
            .arg("--stdio")
            .current_dir(&context)
            .env_clear()
            .env("HOME", &home)
            .env("PATH", "")
            .env("CONNECT_FIXTURE_BEARER", TOKEN)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0)
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let flags = OFlag::from_bits_retain(fcntl(&stdout, FcntlArg::F_GETFL).unwrap());
        fcntl(&stdout, FcntlArg::F_SETFL(flags | OFlag::O_NONBLOCK)).unwrap();
        Self {
            root,
            child: Some(child),
            stdout,
            rows: vec![],
            binary,
            state,
            context,
            home,
        }
    }
    fn send(&mut self, value: Value) {
        use std::io::Write;
        let mut bytes = serde_json::to_vec(&value).unwrap();
        bytes.push(b'\n');
        self.child
            .as_mut()
            .unwrap()
            .stdin
            .as_mut()
            .unwrap()
            .write_all(&bytes)
            .unwrap();
    }
    fn request(&mut self, id: &str, method: &str, params: Value) {
        self.send(json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}));
    }
    fn answer(&mut self, id: &str, answer: Value) {
        self.send(json!({"jsonrpc":"2.0","id":id,"result":{"answer":answer}}));
    }
    async fn next(&mut self) -> Value {
        use std::io::Read;
        let bytes = timeout(BOUND, async {
            let mut bytes = vec![];
            loop {
                let mut byte = [0];
                match self.stdout.read(&mut byte) {
                    Ok(0) => panic!("copied stdio ended before expected frame"),
                    Ok(_) => {
                        if byte[0] == b'\n' {
                            break bytes;
                        }
                        bytes.push(byte[0]);
                        assert!(bytes.len() < 1024 * 1024);
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        sleep(Duration::from_millis(2)).await
                    }
                    Err(e) => panic!("stdio fixture pipe: {e}"),
                }
            }
        })
        .await
        .expect("bounded stdio frame");
        let value: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(value["jsonrpc"], "2.0");
        self.rows.push(value.clone());
        value
    }
    async fn response(&mut self, id: &str) -> Value {
        self.response_since(id, 0).await
    }
    async fn response_since(&mut self, id: &str, offset: usize) -> Value {
        loop {
            if let Some(row) = self.rows[offset..]
                .iter()
                .find(|r| r["id"] == id && r.get("method").is_none())
            {
                return row.clone();
            }
            self.next().await;
        }
    }
    async fn prompt(&mut self, kind: &str) -> Value {
        loop {
            let row = self.next().await;
            if row["method"] == "connect/prompt" {
                assert_eq!(row["params"]["prompt"]["kind"], kind);
                return row;
            }
        }
    }
    async fn finish(&mut self) -> ExitStatus {
        timeout(BOUND, async {
            while self.child.as_mut().unwrap().try_wait().unwrap().is_none() {
                sleep(Duration::from_millis(2)).await;
            }
        })
        .await
        .expect("joined stdio child while stdin remains open");
        let child = self.child.take().unwrap();
        let pid = Pid::from_raw(child.id() as i32);
        let result = child.wait_with_output().unwrap();
        assert_eq!(killpg(pid, None), Err(Errno::ESRCH));
        assert!(
            result.stderr.is_empty(),
            "stdio must keep private diagnostics out of stderr"
        );
        result.status
    }
}
impl Drop for StdioCli {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = killpg(Pid::from_raw(child.id() as i32), Signal::SIGKILL);
            let _ = child.wait();
        }
    }
}

#[tokio::test]
#[ignore = "requires CONNECT_FORGE_TEST_PYTHON and explicit fresh pin-built candidate output"]
async fn actual_stdio_forge_configuration_remount_cancel_and_retire() {
    use meerkat_core::{ApprovalRecord, ApprovalStatus, types::Message};
    use meerkat_runtime::{
        completion::CompletionOutcome, identifiers::LogicalRuntimeId,
        input_state::InputTerminalOutcome,
    };
    const FIRST: &str =
        "Inspect my Reading frontend choices and ask me for the explicit configuration choices.";
    const QUESTION: &str = "The owner offers its participant and lead DM route. Supply your Telegram human ID and acknowledge exclusive polling.";
    const CHOICE: &str = "Use the advertised participant and route, Telegram human ID 123456789 and exclusive polling. Prepare for my private approval.";
    const ANSWER: &str = "The configuration is committed at revision 1. This does not provision a token or prove activation and delivery.";
    const HOLD: &str = "Wait while I review this conversation; do not change any configuration.";
    let mut owner = forge_reading_owner::Owner::start().await;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let (stop, mut stopped) = oneshot::channel();
    let requests = Arc::new(Mutex::new(Vec::<Request>::new()));
    let observed = requests.clone();
    let private_origin = owner.origin.clone();
    let (stream_started, stream_observed) = oneshot::channel();
    let mut stream_started = Some(stream_started);
    let task = tokio::spawn(async move {
        loop {
            let mut stream = tokio::select! {biased;_=&mut stopped=>break,accepted=listener.accept()=>accepted.unwrap().0};
            let received = timeout(BOUND, request(&mut stream)).await.unwrap();
            assert_eq!(received.path, "/v1/chat/completions");
            assert_eq!(received.headers["authorization"], format!("Bearer {TOKEN}"));
            assert_eq!(received.body["model"], "cli-fixture-remote");
            for value in [
                forge_reading_owner::PASSWORD,
                private_origin.as_str(),
                TOKEN,
            ] {
                assert!(!received.body.to_string().contains(value));
            }
            let names: Vec<_> = received.body["tools"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(|t| t["function"]["name"].as_str())
                .collect();
            assert!(names.contains(&"forge_select_host"));
            assert!(!names.iter().any(|name| name.starts_with("paired_host_")
                || *name == "toolkit_prepare_services"
                || *name == "setup_install"));
            let phase = observed.lock().unwrap().len();
            let call = match phase {
                0 => Some(("host", "forge_select_host", json!({}))),
                1 => {
                    assert_eq!(
                        tool_result(&received.body, "host")["frontend_management"],
                        "declared"
                    );
                    Some(("inventory", "forge_frontend_inspect", json!({})))
                }
                2 => {
                    assert_eq!(
                        tool_result(&received.body, "inventory")["observation"]["revision"],
                        0
                    );
                    None
                }
                3 => {
                    let inventory = tool_result(&received.body, "inventory");
                    let source = &inventory["observation"];
                    Some((
                        "prepare",
                        "forge_frontend_prepare",
                        json!({"participant_id":source["participants"][0]["id"],"route_id":source["routes"][0]["id"],"telegram_user_id":123456789,"exclusive_polling_acknowledged":true,"expected_revision":source["revision"]}),
                    ))
                }
                4 => {
                    let prepared = tool_result(&received.body, "prepare");
                    assert_eq!(prepared["approval_required"], true);
                    Some((
                        "submit",
                        "forge_frontend_submit",
                        json!({"change_id":prepared["proposal"]["change_id"]}),
                    ))
                }
                5 => {
                    assert_eq!(
                        tool_result(&received.body, "submit")["observation"]["status"],
                        "committed"
                    );
                    None
                }
                6 => {
                    assert_eq!(
                        received.body["messages"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .filter(|message| message["role"] == "user")
                            .filter_map(|message| message["content"].as_str())
                            .filter(|text| [FIRST, CHOICE, HOLD].contains(text))
                            .collect::<Vec<_>>(),
                        vec![FIRST, CHOICE, HOLD]
                    );
                    None
                }
                _ => panic!("unexpected replay/model call"),
            };
            observed.lock().unwrap().push(received);
            if phase == 6 {
                stream.write_all(b"HTTP/1.1 200 OK\r\nConnection: close\r\nContent-Type: text/event-stream\r\n\r\ndata: {\"choices\":[{\"delta\":{\"content\":\"Waiting\"}}]}\n\n").await.unwrap();
                stream_started.take().unwrap().send(()).unwrap();
                let mut byte = [0];
                tokio::select! {_=&mut stopped=>{},result=stream.read(&mut byte)=>{assert_eq!(result.unwrap(),0);}}
                break;
            }
            let body = if let Some((id, name, args)) = call {
                deferred_call_response(id, name, args)
            } else {
                let answer = if phase == 2 { QUESTION } else { ANSWER };
                format!(
                    "data: {}\n\ndata: [DONE]\n\n",
                    json!({"choices":[{"delta":{"content":answer},"finish_reason":"stop"}]})
                )
            };
            let response = format!(
                "HTTP/1.1 200 OK\r\nConnection: close\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).await.unwrap();
            stream.shutdown().await.unwrap();
        }
    });
    let mut provider = Server {
        origin,
        requests,
        stop: Some(stop),
        task: Some(task),
    };
    let mut cli = StdioCli::start(&provider.origin);
    let config = fs::read(cli.state.join(REALM).join("config.toml")).unwrap();
    cli.request("h:init","connect/initialize",json!({"protocol_version":1,"application":{"application_id":"reading-fixture","instance_id":"actual-instance","origin":owner.origin},"viewer":{"principal_id":"fixture-human","login_scope_id":"actual-login-one","expires_at_ms":std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64+120_000},"realm":{"realm_id":REALM,"state_root":cli.state,"context_root":cli.context,"user_config_root":cli.home},"model":"cli-fixture"}));
    let initialized = cli.response("h:init").await;
    assert!(initialized.get("error").is_none(), "{initialized}");
    let scope = initialized["result"]["scope"].as_str().unwrap().to_owned();
    let session =
        meerkat_core::SessionId::parse(initialized["result"]["session_id"].as_str().unwrap())
            .unwrap();
    assert_eq!(initialized["result"]["capabilities"]["paired_host"], false);
    assert_eq!(
        initialized["result"]["capabilities"]["service_preparation"],
        false
    );
    cli.request(
        "h:first",
        "connect/send",
        json!({"scope":scope,"text":FIRST}),
    );
    let origin_prompt = cli.prompt("forge_origin").await;
    let pid = origin_prompt["id"].as_str().unwrap().to_owned();
    cli.request(
        "h:wrong-scope",
        "connect/history",
        json!({"scope":"other-child"}),
    );
    assert_eq!(
        cli.response("h:wrong-scope").await["error"]["data"]["code"],
        "invalid_binding"
    );
    cli.answer(
        &pid,
        json!({"kind":"forge_origin","origin":"http://127.0.0.1:9"}),
    );
    assert_eq!(
        cli.response(&pid).await["error"]["data"]["code"],
        "invalid_answer"
    );
    cli.answer(&pid, json!({"kind":"forge_origin","origin":owner.origin}));
    let password = cli.prompt("forge_password").await;
    cli.answer(
        password["id"].as_str().unwrap(),
        json!({"kind":"forge_password","password":forge_reading_owner::PASSWORD}),
    );
    assert_eq!(
        cli.response("h:first").await["result"]["completion"],
        "completed"
    );
    cli.request(
        "h:choice",
        "connect/send",
        json!({"scope":scope,"text":CHOICE}),
    );
    let approval = cli.prompt("approval").await;
    let approval_id = approval["params"]["prompt"]["approval_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let approval_request = approval["id"].as_str().unwrap().to_owned();
    assert!(
        approval["params"]["prompt"]["summary"]
            .as_str()
            .unwrap()
            .contains("messages already queued")
    );
    cli.answer("p:wrong", json!({"kind":"approval","decision":"approve"}));
    assert_eq!(
        cli.response("p:wrong").await["error"]["data"]["code"],
        "stale_prompt"
    );
    cli.request("h:remount", "connect/history", json!({"scope":scope}));
    let remount = cli.response("h:remount").await;
    assert_eq!(
        remount["result"]["state"]["active_send"]["send_request_id"],
        "h:choice"
    );
    assert_eq!(
        remount["result"]["state"]["last_completion"]["send_request_id"],
        "h:first"
    );
    assert_eq!(remount["result"]["state"]["can_send"], false);
    assert_eq!(
        cli.rows
            .iter()
            .filter(
                |r| r["method"] == "connect/prompt" && r["params"]["prompt"]["kind"] == "approval"
            )
            .count(),
        1
    );
    assert_eq!(
        owner.control("observe").await["requests"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|r| r["method"] == "POST" && r["path"] == "/console/forge/frontends/submit")
            .count(),
        0
    );
    cli.answer(
        &approval_request,
        json!({"kind":"approval","decision":"approve","actor":"forged"}),
    );
    assert_eq!(
        cli.response(&approval_request).await["error"]["data"]["code"],
        "invalid_answer"
    );
    cli.answer(
        &approval_request,
        json!({"kind":"approval","decision":"approve"}),
    );
    assert_eq!(
        cli.response("h:choice").await["result"]["completion"],
        "completed"
    );
    let duplicate_after = cli.rows.len();
    cli.answer(
        &approval_request,
        json!({"kind":"approval","decision":"approve"}),
    );
    assert_eq!(
        cli.response_since(&approval_request, duplicate_after).await["error"]["data"]["code"],
        "stale_prompt"
    );
    cli.request("h:history", "connect/history", json!({"scope":scope}));
    let history = cli.response("h:history").await;
    assert_eq!(
        history["result"]["state"]["last_completion"],
        json!({"send_request_id":"h:choice","completion":"completed"})
    );
    let projected = history["result"]["history"]["messages"].as_array().unwrap();
    assert_eq!(
        projected
            .iter()
            .filter(|m| m["role"] == "user")
            .map(|m| m["text"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec![FIRST, CHOICE]
    );
    assert!(
        projected
            .iter()
            .any(|m| m["role"] == "assistant" && m["text"] == ANSWER)
    );
    cli.request("h:hold", "connect/send", json!({"scope":scope,"text":HOLD}));
    timeout(BOUND, stream_observed).await.unwrap().unwrap();
    cli.request("h:hold-history", "connect/history", json!({"scope":scope}));
    let held = cli.response("h:hold-history").await;
    assert_eq!(
        held["result"]["state"]["active_send"]["send_request_id"],
        "h:hold"
    );
    assert_eq!(held["result"]["state"]["can_stop"], true);
    cli.request(
        "h:old-cancel",
        "connect/cancel",
        json!({"scope":scope,"send_request_id":"h:first"}),
    );
    assert_eq!(
        cli.response("h:old-cancel").await["result"]["status"],
        "not_current"
    );
    for id in ["h:cancel-one", "h:cancel-two"] {
        cli.request(
            id,
            "connect/cancel",
            json!({"scope":scope,"send_request_id":"h:hold"}),
        );
    }
    cli.request(
        "h:retire",
        "connect/retire",
        json!({"target":{"scope":scope},"retirement_id":"logout","reason":"logout"}),
    );
    cli.request(
        "h:late-send",
        "connect/send",
        json!({"scope":scope,"text":"must never enter"}),
    );
    cli.request(
        "h:close",
        "connect/close",
        json!({"target":{"scope":scope}}),
    );
    assert_eq!(
        cli.response("h:close").await["result"]["resources_closed"],
        true
    );
    assert_eq!(
        cli.response("h:retire").await["result"]["entered_work"],
        "settled"
    );
    assert_eq!(
        cli.response("h:late-send").await["error"]["data"]["code"],
        "binding_retired"
    );
    let cancel_one = cli.response("h:cancel-one").await;
    let cancel_two = cli.response("h:cancel-two").await;
    // Native exact cancellation may archive the terminal input before its
    // final lookup, returning false. Preserve that disposition independently
    // from the actual joined completion below.
    assert!(matches!(
        cancel_one["result"]["status"].as_str(),
        Some("cancellation_requested" | "not_current")
    ));
    assert_eq!(cancel_one["result"]["scope"], scope);
    assert_eq!(cancel_one["result"]["send_request_id"], "h:hold");
    assert_eq!(cancel_one["result"], cancel_two["result"]);
    let held_completion = cli.response("h:hold").await;
    assert!(held_completion.get("error").is_none());
    assert_eq!(held_completion["result"]["completion"], "cancelled");
    let response_position = |id: &str| {
        cli.rows
            .iter()
            .position(|row| row["id"] == id && row.get("method").is_none())
            .unwrap()
    };
    assert!(response_position("h:hold") < response_position("h:retire"));
    assert!(response_position("h:retire") < response_position("h:close"));
    assert!(cli.finish().await.success());
    provider.finish().await;
    let (owner_result, canaries) = owner.finish().await;
    for path in [
        "/console/forge/frontends/prepare",
        "/console/forge/frontends/submit",
    ] {
        assert_eq!(
            owner_result["requests"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|r| r["method"] == "POST" && r["path"] == path)
                .count(),
            1
        );
    }
    assert_eq!(owner_result["state"]["revision"], 1);
    assert_eq!(provider.requests.lock().unwrap().len(), 7);
    assert_eq!(preparation_output_count(&cli.state), 0);
    assert_eq!(
        fs::read(cli.state.join(REALM).join("config.toml")).unwrap(),
        config
    );
    let artifact_root = cli.state.join(".meerkat-connect").join(REALM);
    let mut decisions = vec![];
    for setup in fs::read_dir(artifact_root).unwrap() {
        for entry in fs::read_dir(setup.unwrap().path()).unwrap() {
            let path = entry.unwrap().path().join("native-decisions.json");
            if path.is_file() {
                let value: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
                decisions.extend(
                    serde_json::from_value::<Vec<ApprovalRecord>>(value["approvals"].clone())
                        .unwrap(),
                );
            }
        }
    }
    assert_eq!(decisions.len(), 1);
    assert_eq!(decisions[0].status, ApprovalStatus::Approved);
    assert_eq!(decisions[0].approval_id.to_string(), approval_id);
    assert_eq!(
        decisions[0].decision.as_ref().unwrap().actor.as_str(),
        "fixture-human"
    );
    let storage = meerkat_connect::RealmOptions {
        context_root: cli.context.clone(),
        state_root: Some(cli.state.clone()),
        user_config_root: Some(cli.home.clone()),
    }
    .resolve(REALM)
    .unwrap();
    let (_, persistence) = meerkat::storage_provider::open_realm_persistence_with_layout(
        storage.layout,
        REALM,
        None,
        Some(meerkat_store::RealmOrigin::Explicit),
    )
    .await
    .unwrap();
    let stored = persistence
        .session_store()
        .load(&session)
        .await
        .unwrap()
        .unwrap();
    let users = stored
        .messages()
        .iter()
        .filter_map(|m| {
            if let Message::User(u) = m {
                Some(u.text_content())
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    // The cancelled turn has an admitted input and exact completion below,
    // but its draft user message is not committed to canonical history.
    assert_eq!(users, vec![FIRST, CHOICE]);
    let mut inputs = persistence
        .runtime_store()
        .load_input_states_strict(&LogicalRuntimeId::for_session(&session))
        .await
        .unwrap();
    assert_eq!(inputs.len(), 3);
    inputs.sort_by_key(|input| {
        input
            .seed
            .admission_sequence
            .expect("native admission sequence")
    });
    let mut native_completions = vec![];
    for (input, send_id) in inputs.iter().zip(["h:first", "h:choice", "h:hold"]) {
        let value = serde_json::to_value(input).unwrap();
        let outcome: CompletionOutcome =
            serde_json::from_value(value["terminal_completion"]["outcome"].clone()).unwrap();
        let outcome = serde_json::to_value(outcome).unwrap();
        let wire = cli
            .rows
            .iter()
            .find(|row| row["id"] == send_id && row.get("method").is_none())
            .unwrap();
        assert_eq!(outcome["completion_type"], wire["result"]["completion"]);
        native_completions.push(outcome["completion_type"].clone());
        assert!(matches!(
            input.seed.terminal_outcome,
            Some(InputTerminalOutcome::Consumed | InputTerminalOutcome::Abandoned { .. })
        ));
        assert_eq!(input.seed.attempt_count, 1);
        assert_eq!(input.state.recovery_count, 0);
    }
    let public_history = serde_json::to_string(stored.messages()).unwrap();
    let calls = provider.requests.lock().unwrap();
    for secret in canaries.iter().map(String::as_str).chain([
        forge_reading_owner::PASSWORD,
        owner.origin.as_str(),
        approval_id.as_str(),
    ]) {
        assert!(!public_history.contains(secret));
        for call in calls.iter() {
            assert!(!call.body.to_string().contains(secret));
        }
        for row in &cli.rows {
            if row["method"] != "connect/prompt" {
                assert!(!row.to_string().contains(secret));
            }
        }
    }
    println!(
        "Stdio Forge composition: {}",
        json!({"scope":"copied_stdio_native_frozen_owner_scripted_model","candidate_binary":cli.binary,"cli_sha256":file_digest(&cli.binary),"test_sha256":file_digest(&std::env::current_exe().unwrap()),"resources_sha256":option_env!("MEERKAT_CONNECT_RESOURCES_SHA256"),"native_sessions":1,"native_inputs":3,"native_completions":native_completions,"cancel_status":cancel_one["result"]["status"],"completion_before_retire_before_close":true,"prepare_posts":1,"submit_posts":1,"revision":1,"service_preparations":0,"closed_with_stdin_open":true,"live_activation":false,"all_owned_processes_joined":true})
    );
    let _ = &cli.root;
}
