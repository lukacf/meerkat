//! Live public-read acceptance, through the supplied package and ordinary rkat.
//! The LLM is scripted on loopback; the Hacker News operation is not simulated.
//! Its selected Agno tool owns the fixed public Firebase HTTPS URLs. This does
//! not establish private account authentication or arbitrary package readiness.
use super::*;
use super::profile_denial::{optional_result, package_response, package_response_scoped};
use std::os::unix::fs::OpenOptionsExt;

const OPERATION: &str = "get_top_hackernews_stories";
const SECOND_OPERATION: &str = "search_wikipedia";
const SECOND_PREFIX: &str = "second-";
const SECOND_DONE: &str = "The second exact public service access was reviewed. Its installation and the prior Hacker News job were read back in this continued setup conversation; no service operation has run.";
const SETUP_PROMPT: &str = "Install the supplied Agno Hacker News integration for get_top_hackernews_stories, then install the supplied Agno Wikipedia integration for search_wikipedia with language en. Ask for access for rkat-agent until revoked for each service, and finish both setups without calling either service.";
const SETUP_DONE: &str = "The exact public service access was reviewed. Setup is closing for finalization; no service operation has run yet.";
const CONTINUED: &str = "Both retained native results confirm configuration and selected access. The second service was acquired in the same continued setup invocation, and both exact jobs were read back without replaying installation; no service operation has run.";
const RUN_PROMPT: &str = "Use the installed get_top_hackernews_stories tool with num_stories set to 3, then report all three returned stories with their ids and titles.";
const ACCESS_CALL: &str = "review-public-access";
const STORY_CALL: &str = "fresh-rkat-public-story";
const SEARCH_CALL: &str = "fresh-rkat-public-search";
const LOAD_CALL: &str = "fresh-rkat-public-load";
const ACCESS_REVIEW: &str = "Enable this public service?";

fn completed(text: &str) -> String {
    let event = json!({"choices":[{"delta":{"content":text},"finish_reason":"stop"}]});
    format!("data: {event}\n\ndata: {{\"choices\":[],\"usage\":{{\"prompt_tokens\":100,\"completion_tokens\":20,\"total_tokens\":120}}}}\n\ndata: [DONE]\n\n")
}

fn public_access(request: &Request, job_id: &str, raw: &str) -> String {
    public_access_scoped(request, job_id, raw, OPERATION, "", SETUP_DONE)
}

fn second_public_access(request: &Request, job_id: &str, raw: &str) -> String {
    let body = &request.body;
    let first = tool_result(body, "reconciled-install");
    let first_job = first["observation"]["job_id"].as_str().unwrap();
    assert_ne!(first_job, job_id, "distinct actual jobs across native helper lifetimes");
    if optional_result(body, "second-read-prior-install").is_none() {
        assert!(body["tools"].as_array().unwrap().iter()
            .any(|tool| tool["function"]["name"] == "setup_reconcile"));
        return deferred_call_response("second-read-prior-install", "setup_reconcile", json!({"job_id":first_job}));
    }
    let reread = tool_result(body, "second-read-prior-install");
    assert_eq!(reread["observation"]["job_id"], first_job);
    assert_eq!(reread["observation"]["status"], "matched");
    assert_eq!(reread["observation"]["result"], first["observation"]["result"],
        "the new owner must read the exact prior installation, not regenerate its receipt");
    public_access_scoped(request, job_id, raw, SECOND_OPERATION, SECOND_PREFIX, SECOND_DONE)
}

fn public_access_scoped(request: &Request, job_id: &str, raw: &str,
    operation: &str, prefix: &str, done: &str) -> String {
    let body = &request.body;
    let id = |suffix: &str| format!("{prefix}{suffix}");
    assert_eq!(raw, operation, "actual Agno publisher raw identity");
    assert_eq!(tool_result(body, &id("current-operation-names"))["authentication"], json!({"kind":"none"}));
    assert_eq!(tool_result(body, &id("checked-install"))["credential_environment_references"], json!([]));
    if optional_result(body, &id(ACCESS_CALL)).is_none() {
        assert!(body["tools"].as_array().unwrap().iter()
            .any(|tool| tool["function"]["name"] == "toolkit_enable_public_service"));
        return deferred_call_response(&id(ACCESS_CALL), "toolkit_enable_public_service", json!({
            "job_id":job_id,"operations":[raw],"agent":"rkat-agent","expires_in_ms":null,
        }));
    }
    let access = tool_result(body, &id(ACCESS_CALL));
    assert_eq!(access["state"], "pending");
    assert_eq!(access["reason"], "reviewed_finalization_waiting_for_setup_close");
    assert_eq!(access["access_reviewed"], true);
    assert_eq!(access["grant_issued"], false);
    completed(done)
}

#[derive(Debug, serde::Deserialize, serde::Serialize)]
struct Story {
    id: u64,
    title: String,
}

// The retained MCP projection preserves both Agno's text JSON array and its
// distinct structured {value: <the same JSON string>} block. Parse those exact
// values, not a substring that could turn an error envelope into success.
fn public_stories(content: &str) -> Result<Vec<Story>, String> {
    let values: Vec<Value> = serde_json::Deserializer::from_str(content).into_iter()
        .collect::<Result<_, _>>().map_err(|error| error.to_string())?;
    if values.len() != 2 || !values[0].is_array()
        || values[1].as_object().is_none_or(|object| object.len() != 1)
    { return Err("expected the real Agno text and structured result pair".into()); }
    let mirrored: Value = serde_json::from_str(values[1]["value"].as_str()
        .ok_or("missing Agno structured value")?).map_err(|error| error.to_string())?;
    if mirrored != values[0] { return Err("MCP text and structured values disagree".into()); }
    let stories: Vec<Story> = serde_json::from_value(mirrored).map_err(|error| error.to_string())?;
    if stories.len() != 3 { return Err("the live operation must return three stories".into()); }
    if stories.iter().any(|story| story.id == 0 || story.title.trim().is_empty()) {
        return Err("empty public story".into());
    }
    if stories.iter().map(|story| story.id).collect::<BTreeSet<_>>().len() != 3 {
        return Err("the live operation must return three distinct stories".into());
    }
    Ok(stories)
}

fn story_answer(stories: &[Story]) -> String {
    stories.iter().map(|story| format!("Hacker News story {}: {}", story.id, story.title))
        .collect::<Vec<_>>().join("\n")
}

// Decode observed display data only. This is the finalizer's known no-media
// SystemNotice shape, rendered by native code; no Input or authority is made.
fn finalization_projection(outcome: &Value) -> String {
    use meerkat_core::types::{SystemNoticeBlock, SystemNoticeKind, SystemNoticeMessage};
    let body = outcome.to_string();
    SystemNoticeMessage::with_blocks(
        SystemNoticeKind::ExternalEvent,
        Some(body.clone()),
        vec![SystemNoticeBlock::ExternalEvent {
            source: "meerkat-connect-finalization".into(),
            event_type: "connect_finalization_observation".into(),
            summary: Some(body.clone()),
            body: Some(body.clone()),
            payload: Some(json!({"body":body})),
            content: Vec::new(),
        }],
    ).model_projection_text()
}

fn finalization_notices(messages: &[Value]) -> Result<Vec<Value>, &'static str> {
    let mut outcomes = Vec::new();
    let mut seen = BTreeSet::new();
    for message in messages.iter().filter(|message| message["role"] == "user") {
        let Some(content) = message["content"].as_str() else { continue; };
        // The native text starts with the JSON body and then its event block.
        // Parsing that first value identifies a candidate; full equality below
        // rejects partial, changed, foreign-source or trailing representations.
        let Some(Ok(outcome)) = serde_json::Deserializer::from_str(content)
            .into_iter::<Value>().next() else { continue; };
        if outcome["kind"] != "connect_finalization_observation" { continue; }
        if content != finalization_projection(&outcome) {
            return Err("finalization text is not the complete native projection");
        }
        if !seen.insert(content) {
            return Err("the same finalization projection appeared more than once");
        }
        outcomes.push(outcome);
    }
    Ok(outcomes)
}

// Native ToolScope notices are user-role context on this provider surface.
// Re-render the complete typed display; its text does not prove provenance or
// supply input authority. Startup diff bits/revision are not fixed defaults.
fn tool_scope_projection(base_changed: bool, visible_changed: bool, revision: u64) -> String {
    use meerkat_core::event::{ToolConfigChangeDomain, ToolConfigChangeOperation,
        ToolConfigChangeStatus, ToolConfigChangedPayload};
    use meerkat_core::types::{SystemNoticeBlock, SystemNoticeKind, SystemNoticeMessage};
    let status = ToolConfigChangeStatus::boundary_applied(base_changed, visible_changed, revision);
    let body = format!("Tool configuration changed at turn boundary: {}", status.status_text());
    let payload = ToolConfigChangedPayload::new(
        ToolConfigChangeOperation::Reload, "tool_scope", status, false,
    ).with_domain(Some(ToolConfigChangeDomain::ToolScope));
    SystemNoticeMessage::with_block(SystemNoticeKind::ToolScope, Some(body),
        SystemNoticeBlock::ToolConfig { payload }).model_projection_text()
}

fn tool_scope_display_fields(content: &str) -> Option<(bool, bool, u64)> {
    let summary = content.lines().next()?;
    let mut fields = summary.strip_prefix(
        "Tool configuration changed at turn boundary: boundary_applied("
    )?.strip_suffix(')')?.split(',');
    let base: bool = fields.next()?.strip_prefix("base_changed=")?.parse().ok()?;
    let visible: bool = fields.next()?.strip_prefix("visible_changed=")?.parse().ok()?;
    let revision: u64 = fields.next()?.strip_prefix("revision=")?.parse().ok()?;
    if fields.next().is_some() || !(base || visible)
        || content != tool_scope_projection(base, visible, revision)
    { return None; }
    Some((base, visible, revision))
}

// Render native display from supplied groups. Acceptance below supplies only
// the exact source-bound set; the diagnostic parser never supplies authority.
// The second group is the known loaded set leaving hidden catalog membership.
// Actual callable readiness is checked from typed load results and offered tools.
fn deferred_catalog_projection(groups: &[Vec<&str>; 3]) -> String {
    use meerkat_core::event::{ToolConfigChangeDomain, ToolConfigChangeOperation,
        ToolConfigChangeStatus, ToolConfigChangedPayload};
    use meerkat_core::types::{SystemNoticeBlock, SystemNoticeKind, SystemNoticeMessage};
    let labels = ["new deferred tools available: ", "deferred tools now available to call: ",
        "sources still connecting: "];
    let clauses: Vec<_> = groups.iter().zip(labels).filter(|(names, _)| !names.is_empty())
        .map(|(names, label)| format!("{label}{}", names.join(", "))).collect();
    let status = ToolConfigChangeStatus::deferred_catalog_delta(
        groups[0].len(), groups[1].len(), groups[2].len());
    let payload = ToolConfigChangedPayload::new(
        ToolConfigChangeOperation::Reload, "deferred_catalog", status, false,
    ).with_domain(Some(ToolConfigChangeDomain::DeferredCatalog));
    SystemNoticeMessage::with_block(SystemNoticeKind::ToolScope,
        Some(format!("Deferred catalog changed at turn boundary: {}", clauses.join("; "))),
        SystemNoticeBlock::ToolConfig { payload }).model_projection_text()
}

fn deferred_catalog_display_fragments(content: &str) -> Option<Value> {
    // Refuse ambiguous delimiters and noncanonical lookalikes.
    // All limits apply before any fragment is included in the failure output.
    if content.len() > 4096 { return None; }
    let (body, _) = content.split_once('\n')?;
    let body = body.strip_prefix("Deferred catalog changed at turn boundary: ")?;
    let labels = ["new deferred tools available: ", "deferred tools now available to call: ",
        "sources still connecting: "];
    let mut groups: [Vec<&str>; 3] = std::array::from_fn(|_| Vec::new());
    let mut previous = None;
    let mut total_count = 0;
    let mut total_bytes = 0;
    for clause in body.split("; ") {
        let (index, names) = labels.iter().enumerate()
            .find_map(|(index, label)| clause.strip_prefix(*label).map(|names| (index, names)))?;
        if previous.is_some_and(|prior| index <= prior) { return None; }
        previous = Some(index);
        for name in names.split(", ") {
            if name.is_empty() || name.len() > 128 || !name.bytes().all(|byte|
                byte.is_ascii_alphanumeric() || b"_.:/-".contains(&byte))
            { return None; }
            total_count += 1;
            total_bytes += name.len();
            if total_count > 16 || total_bytes > 1024 { return None; }
            groups[index].push(name);
        }
        if !groups[index].windows(2).all(|pair| pair[0] < pair[1]) { return None; }
    }
    if groups[0].iter().any(|name| groups[1].contains(name))
        || content != deferred_catalog_projection(&groups)
    { return None; }
    Some(json!({"added_hidden_count":groups[0].len(),
        "removed_hidden_count":groups[1].len(),"pending_source_count":groups[2].len(),
        "added_display_fragments":groups[0],"removed_display_fragments":groups[1],
        "pending_display_fragments":groups[2]}))
}

fn diagnostic_json_kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "null", Value::Bool(_) => "boolean", Value::Number(_) => "number",
        Value::String(_) => "string", Value::Array(_) => "array", Value::Object(_) => "object",
    }
}

fn fresh_text_shape(text: &str) -> Value {
    // Display-prefix hints are diagnostics only. They neither prove a native
    // origin nor change the complete-renderer acceptance check below.
    let hint = [
        ("Tool configuration changed at turn boundary: ", "tool_scope_boundary"),
        ("Deferred catalog changed at turn boundary: ", "deferred_catalog"),
        ("Tool scope apply failed (", "tool_scope_warning"),
        ("Servers connecting: ", "mcp_pending"),
        ("An MCP server could not be started or reloaded.", "mcp_setup_failure"),
        ("An MCP server could not be started or reloaded: ", "mcp_setup_failure"),
    ].into_iter().find_map(|(prefix, hint)| text.starts_with(prefix).then_some(hint))
        .unwrap_or("unknown");
    json!({"bytes":text.len(),"sha256":format!("{:x}", Sha256::digest(text.as_bytes())),
        "display_prefix_hint":hint,"deferred_catalog_display":deferred_catalog_display_fragments(text)})
}

fn fresh_content_shape(content: &Value) -> Value {
    let parts = content.as_array().map(|parts| parts.iter().take(8).map(|part| {
        let part_type = match part.get("type").and_then(Value::as_str) {
            Some("text") => "text", Some("image_url") => "image_url", _ => "other",
        };
        let text = part.as_str().or_else(|| part.get("text").and_then(Value::as_str));
        json!({"json_kind":diagnostic_json_kind(part),"part_type":part_type,
            "text_kind":part.get("text").map(diagnostic_json_kind).unwrap_or("missing"),
            "text":text.map(fresh_text_shape)})
    }).collect::<Vec<_>>());
    json!({"json_kind":diagnostic_json_kind(content),
        "text":content.as_str().map(fresh_text_shape),
        "part_count":content.as_array().map(Vec::len),"parts":parts})
}

fn fresh_input_shape(messages: &[Value]) -> Value {
    // Failure-only fixture evidence: no raw content, headers or unknown role
    // strings. Bound the row count even if the rejected history is large.
    let rows: Vec<_> = messages.iter().take(16).map(|message| {
        let role = match message["role"].as_str() {
            Some("system") => "system", Some("user") => "user",
            Some("assistant") => "assistant", Some("tool") => "tool", _ => "other",
        };
        let bytes = serde_json::to_vec(&message["content"]).unwrap();
        let boundary = message["content"].as_str().and_then(tool_scope_display_fields)
            .map(|(base, visible, revision)| json!({"base_changed":base,
                "visible_changed":visible,"revision":revision}));
        json!({"role":role,"content_json_bytes":bytes.len(),
            "content_json_sha256":format!("{:x}", Sha256::digest(&bytes)),
            "business_prompt":message["content"] == RUN_PROMPT,
            "boundary_applied":boundary,"content_shape":fresh_content_shape(&message["content"])})
    }).collect();
    json!({"message_count":messages.len(),"messages":rows})
}

// The two reviewed service descriptors and the saved setup server determine
// this exact initial set. The pinned local-session owner reopens without live
// setup custody as readback-only; its adapter exposes these five recovery reads.
// This display comparison grants nothing and never makes installation callable.
fn fresh_deferred_addition() -> String {
    deferred_catalog_projection(&[vec![OPERATION, SECOND_OPERATION, "setup_job", "setup_jobs",
        "setup_reconcile", "setup_recovery_manifest", "setup_state"], vec![], vec![]])
}

fn fresh_native_context(messages: &[Value]) -> bool {
    let business: Vec<_> = messages.iter().enumerate()
        .filter(|(_, message)| message["role"] == "user" && message["content"] == RUN_PROMPT)
        .map(|(index, _)| index).collect();
    if business.len() != 1 { return false; }
    let loads: Vec<_> = messages.iter().enumerate()
        .filter(|(_, message)| message["role"] == "tool" && message["tool_call_id"] == LOAD_CALL)
        .map(|(index, _)| index).collect();
    if loads.len() > 1 { return false; }
    let loaded_at = loads.first().copied().unwrap_or(messages.len());
    let added_text = fresh_deferred_addition();
    let loaded_text = deferred_catalog_projection(&[vec![], vec![OPERATION], vec![]]);
    let (mut added, mut loaded, mut boundaries) = (false, false, [false; 2]);
    for (index, message) in messages.iter().enumerate().filter(|(_, row)| row["role"] == "user") {
        if index == business[0] { continue; }
        if index < business[0] { return false; }
        let Some(content) = message["content"].as_str() else { return false; };
        let after_load = index > loaded_at;
        if tool_scope_display_fields(content).is_some() {
            let segment = usize::from(after_load);
            if boundaries[segment] { return false; }
            boundaries[segment] = true;
        } else if content == added_text {
            if added || after_load { return false; }
            added = true;
        } else if content == loaded_text {
            if !added || loaded || !after_load { return false; }
            loaded = true;
        } else { return false; }
    }
    true
}

fn assert_fresh_business_input(messages: &[Value]) {
    assert!(fresh_native_context(messages),
        "fresh input must contain one business prompt and only the exact bounded native catalog context, without setup/finalization/other user history: {}",
        fresh_input_shape(messages));
}

fn fresh_call(index: usize) -> (&'static str, &'static str, Value) {
    match index {
        0 => (SEARCH_CALL, "tool_catalog_search", json!({"query":OPERATION,"limit":50})),
        1 => (LOAD_CALL, "tool_catalog_load", json!({"names":[OPERATION]})),
        2 => (STORY_CALL, OPERATION, json!({"num_stories":3})),
        _ => panic!("no further fresh calls are permitted"),
    }
}

// Join every actual call/result pair in order. Catalog work is metadata only;
// exactly one subsequent service effect is allowed, without setup/history replay.
fn fresh_results(messages: &[Value]) -> Vec<&Value> {
    let (mut results, mut pending) = (Vec::new(), false);
    for message in messages {
        match message["role"].as_str() {
            Some("system" | "user") => (),
            Some("assistant") => {
                assert!(!pending && results.len() < 3, "unexpected or replayed fresh call");
                let (id, name, arguments) = fresh_call(results.len());
                assert!(message["content"].is_null() && message["tool_calls"] == json!([
                    {"id":id,"type":"function","function":{"name":name,
                        "arguments":arguments.to_string()}}]), "fresh call differs from its exact selected request");
                pending = true;
            }
            Some("tool") => {
                assert!(pending && results.len() < 3, "fresh result lacks its unique preceding call");
                assert!(message["tool_call_id"] == fresh_call(results.len()).0,
                    "fresh result belongs to another call");
                results.push(message);
                pending = false;
            }
            _ => panic!("unexpected fresh message role"),
        }
    }
    assert!(!pending, "fresh native request contains an unsettled tool call");
    results
}

fn assert_fresh_catalog(results: &[&Value]) {
    if let Some(message) = results.first() {
        let found: Value = serde_json::from_str(message["content"].as_str()
            .expect("native search result must be text JSON")).unwrap_or_else(|_| panic!("native search result JSON"));
        let matches: Vec<_> = found["results"].as_array().expect("native search rows").iter()
            .filter(|row| row["name"] == OPERATION).collect();
        assert!(found["catalog_exact"] == true
            && found.get("pending_sources").is_none_or(|pending| pending == &json!([]))
            && found["empty_result_status"].is_null() && matches.len() == 1
            && matches!(matches[0]["visibility_status"].as_str(), Some("loaded" | "deferred")),
            "native search did not locate the exact available service operation");
    }
    if let Some(message) = results.get(1) {
        let loaded: Value = serde_json::from_str(message["content"].as_str()
            .expect("native load result must be text JSON")).unwrap_or_else(|_| panic!("native load result JSON"));
        let accepted = loaded["accepted_names"] == json!([OPERATION]) && loaded["noop_names"] == json!([]);
        let noop = loaded["accepted_names"] == json!([]) && loaded["noop_names"] == json!([OPERATION]);
        let resolutions: Vec<meerkat_core::ToolCatalogLoadResolution> =
            serde_json::from_value(loaded["resolutions"].clone())
                .unwrap_or_else(|_| panic!("typed native load resolutions"));
        assert!(loaded["catalog_exact"] == true && (accepted || noop) && resolutions.len() == 1
            && resolutions[0].name == OPERATION && resolutions[0].accepted
            && resolutions[0].rejected_reason.is_none() && resolutions[0].accepted_noop == noop,
            "native load did not accept exactly the selected service operation");
    }
}

fn response(request: &Request, phase: usize) -> String {
    let messages = request.body["messages"].as_array().unwrap();
    let fresh = messages.iter().any(|message|
        message["role"] == "user" && message["content"] == RUN_PROMPT);
    if fresh {
        // Refuse mixed fresh/setup history before setup-finalization routing.
        assert_fresh_business_input(messages);
    }
    let outcomes = finalization_notices(messages)
        .expect("complete native finalization projections without replay");
    if !outcomes.is_empty() {
        assert!(outcomes.len() <= 2, "one native outcome occurrence per finalizer, no replay");
        for outcome in &outcomes {
            assert_eq!(outcome["kind"], "connect_finalization_observation");
            assert_eq!(outcome["realm"], REALM);
            assert_eq!(outcome["consumer_agent"], "rkat-agent");
            assert_eq!(outcome["facts"]["persistence"], "saved");
            assert_eq!(outcome["facts"]["grant"]["state"], "selected");
            assert_eq!(outcome["service_readiness"], "not_observed");
            assert_eq!(outcome["automatic_reentry_blocked"], false);
        }
        assert!(messages.iter().any(|message| message["role"] == "user" && message["content"] == SETUP_PROMPT),
            "same durable setup history survives native close and reopen");
        assert!(messages.iter().any(|message| message["role"] == "assistant" && message["content"] == SETUP_DONE));
        let first = tool_result(&request.body, "reconciled-install");
        assert_eq!(outcomes[0]["source_job"], first["observation"]["job_id"]);
        assert_eq!(outcomes[0]["server"], reviewed_descriptor(&request.body, "", "hackernews", OPERATION).name);
        if outcomes.len() == 1 {
            return package_response_scoped(request, phase, SETUP_PROMPT,
                json!({"family":"agno","profile":"wikipedia","operations":[SECOND_OPERATION],
                    "values":{"timeout":7,"language":"en"}}),
                SECOND_OPERATION, false, SECOND_PREFIX, second_public_access);
        }
        let second = tool_result(&request.body, "second-reconciled-install");
        assert_eq!(outcomes[1]["source_job"], second["observation"]["job_id"]);
        assert_eq!(outcomes[1]["server"], reviewed_descriptor(&request.body, SECOND_PREFIX, "wikipedia", SECOND_OPERATION).name);
        assert_ne!(outcomes[0]["source_job"], outcomes[1]["source_job"]);
        assert_ne!(outcomes[0]["server"], outcomes[1]["server"]);
        assert!(messages.iter().any(|message| message["role"] == "assistant" && message["content"] == SECOND_DONE));
        assert_eq!(tool_result(&request.body, "second-read-prior-install")["observation"]["result"],
            first["observation"]["result"]);
        return completed(CONTINUED);
    }
    if !fresh {
        return package_response(request, phase, SETUP_PROMPT,
            json!({"family":"agno","profile":"hackernews","operations":[OPERATION],"values":{"timeout":7}}),
            OPERATION, false, public_access);
    }
    assert_eq!(request.method, "POST");
    assert_eq!(request.path, "/v1/chat/completions");
    assert_eq!(request.headers["authorization"], format!("Bearer {TOKEN}"));
    assert_eq!(request.body["model"], "cli-fixture-remote");
    assert!(phase < 64, "bounded two-service setup plus one ordinary service call");
    assert!(!request.body.to_string().contains(SETUP_PROMPT), "fresh ordinary session, not setup resume");
    assert!(!request.body.to_string().contains(TOKEN));
    let results = fresh_results(messages);
    assert_fresh_catalog(&results);
    let tools = request.body["tools"].as_array().unwrap();
    for checked_only in ["setup_install", "setup_selected_check", "setup_selected_install"] {
        assert!(!tools.iter().any(|tool| tool["function"]["name"] == checked_only),
            "fresh ordinary rkat must not offer a closed checked-installation operation");
    }
    if results.len() < 3 {
        let (id, name, arguments) = fresh_call(results.len());
        assert!(tools.iter().filter(|tool| tool["function"]["name"] == name).count() == 1,
            "the exact next catalog or service operation is not offered");
        return deferred_call_response(id, name, arguments);
    }
    let stories = public_stories(results[2]["content"].as_str().unwrap())
        .expect("the actual public service must return three typed distinct stories");
    completed(&story_answer(&stories))
}

/// This is the same test-owned PTY/process cleanup owner, reusing its roots and
/// persisted config. It never grants access, rewrites config or reopens Connect.
impl Cli {
    fn start_fresh_rkat(&mut self, binary: &Path, expected_sha256: &str) -> PathBuf {
        assert!(self.settled && self.child.try_wait().unwrap().is_some());
        let setup_pid = self.child.id();
        assert_eq!(killpg(Pid::from_raw(setup_pid as i32), None), Err(Errno::ESRCH),
            "the old Connect process group must be gone first");
        assert!(binary.is_absolute() && fs::symlink_metadata(binary).unwrap().is_file());
        assert_eq!(file_digest(binary), expected_sha256, "exact supplied final rkat artifact");
        let root = self._root.as_ref().unwrap().path();
        let output = root.join("fresh-rkat.stdout.json");
        let stdout = fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&output).unwrap();
        let initial_config = fs::read(self.state.join(REALM).join("config.toml")).unwrap();
        let previous_preparations = preparation_artifacts(&self.state).into_iter().collect();
        let mut command = Command::new(binary);
        command.args(["--realm", REALM, "--state-root"]).arg(&self.state)
            .arg("--context-root").arg(root.join("outside-checkout"))
            .arg("--user-config-root").arg(root.join("home"))
            .args(["run", "--model", "cli-fixture", "--provider", "self-hosted",
                "--output", "json", "--no-stream", "--stdin", "off",
                "--wait-for-mcp", "--mcp-auth", "stored", RUN_PROMPT])
            // No --as-agent: this acceptance uses ordinary rkat's default principal.
            .env_clear().env("HOME", root.join("home")).env("PATH", root.join("empty-path"))
            .env("TMPDIR", root).env("CONNECT_FIXTURE_BEARER", TOKEN)
            .current_dir(root.join("outside-checkout")).process_group(0)
            .stdin(Stdio::from(self.observer.try_clone().unwrap()))
            .stdout(Stdio::from(stdout)).stderr(Stdio::from(self.observer.try_clone().unwrap()));
        let child = command.spawn().unwrap();
        self.child = child;
        self.settled = false;
        self.transcript.clear();
        self.initial_config = initial_config;
        self.previous_preparations = previous_preparations;
        self.setup_approvals.clear();
        self.denied_setup_review = None;
        assert_ne!(self.child.id(), setup_pid);
        output
    }
}

fn reviewed_descriptor(body: &Value, prefix: &str, profile: &str, operation: &str) -> meerkat_core::McpServerConfig {
    let readback = tool_result(body, &format!("{prefix}reconciled-install"));
    let result = &readback["observation"]["result"];
    assert_eq!(result["family"], "agno");
    assert_eq!(result["profile"], profile);
    let document = result["mcp_configuration"].as_object().unwrap();
    assert_eq!(document.len(), 1);
    let servers = document["mcpServers"].as_object().unwrap();
    assert_eq!(servers.len(), 1);
    let (name, launch) = servers.iter().next().unwrap();
    let stdio: meerkat_core::mcp_config::McpStdioConfig = serde_json::from_value(launch.clone()).unwrap();
    assert!(Path::new(&stdio.command).is_absolute() && stdio.env.is_empty());
    let mut server = meerkat_core::McpServerConfig::stdio(name, stdio.command, stdio.args, stdio.env);
    server.access = Some(meerkat_core::McpServerAccess::allowing([operation]));
    server
}

// Read process identities only, never arguments or environment. Native MCP
// helpers own separate groups, so absence of Connect's group alone is not proof
// of their exit. Unseen descendants remain covered by native close controls.
#[derive(Clone, Copy, Debug)]
struct ObservedProcess { pid: u32, parent: u32, group: u32 }

async fn process_identities() -> Vec<ObservedProcess> {
    let output = timeout(BOUND, tokio::process::Command::new("/bin/ps")
        .args(["-axo", "pid=,ppid=,pgid="]).env_clear().env("LC_ALL", "C")
        .stdin(Stdio::null()).kill_on_drop(true).output()).await.unwrap().unwrap();
    assert!(output.status.success(), "process identity observation must succeed");
    assert!(output.stdout.len() <= 1024 * 1024);
    std::str::from_utf8(&output.stdout).unwrap().lines().map(|line| {
        let values: Vec<u32> = line.split_whitespace().map(|value| value.parse().unwrap()).collect();
        assert_eq!(values.len(), 3);
        ObservedProcess { pid:values[0], parent:values[1], group:values[2] }
    }).collect()
}

async fn observed_setup_helpers(cli: &mut Cli) -> Vec<ObservedProcess> {
    assert!(cli.child.try_wait().unwrap().is_none());
    let processes = process_identities().await;
    let mut descendants = BTreeSet::from([cli.child.id()]);
    loop {
        let before = descendants.len();
        for process in &processes {
            if descendants.contains(&process.parent) { descendants.insert(process.pid); }
        }
        if before == descendants.len() { break; }
    }
    let observed: Vec<_> = processes.into_iter()
        .filter(|process| process.pid != cli.child.id() && descendants.contains(&process.pid)).collect();
    assert!(observed.iter().any(|process| process.group != cli.child.id()),
        "a real setup helper in its own native process group must be observed during review");
    observed
}

async fn assert_observed_helpers_gone(observed: &[ObservedProcess]) {
    let current = process_identities().await;
    for old in observed {
        assert!(!current.iter().any(|process| process.pid == old.pid),
            "observed setup helper remains after Connect closed: {old:?}");
    }
    for group in observed.iter().map(|process| process.group).collect::<BTreeSet<_>>() {
        assert_eq!(killpg(Pid::from_raw(i32::try_from(group).unwrap()), None), Err(Errno::ESRCH),
            "an observed setup helper process group remains after Connect closed");
    }
}

pub(super) async fn run() {
    // The same settled realm may be kept for a separate live-model consumer.
    // This never changes the controlled result's evidence scope.
    let keep_realm = std::env::var_os("CONNECT_KEEP_FRESH_REALM").map(|value| {
        assert_eq!(value, "1", "CONNECT_KEEP_FRESH_REALM must be exactly 1");
        true
    }).unwrap_or(false);
    // These are mandatory when this ignored live test is explicitly selected.
    // The delivery owner supplies the hash from the accepted native tuple; this
    // fixture checks bytes and must not infer tuple provenance from a filename.
    let binary = PathBuf::from(std::env::var_os("CONNECT_FINAL_RKAT")
        .expect("supply the accepted exact-final rkat binary, not an interim build"));
    let expected_sha256 = std::env::var("CONNECT_FINAL_RKAT_SHA256")
        .expect("supply the accepted native delivery's rkat SHA-256");
    assert_eq!(expected_sha256.len(), 64);
    assert!(expected_sha256.bytes().all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)));
    assert!(binary.is_absolute() && fs::symlink_metadata(&binary).unwrap().is_file());
    assert_eq!(file_digest(&binary), expected_sha256);
    let prefix = PathBuf::from(std::env::var_os("CONNECT_REAL_LIBRARY_PREFIX")
        .expect("supply the isolated actual library with this exact pin-built Connect executable"));
    assert!(prefix.is_absolute());
    let mut provider = Server::package_provider(response).await;
    let root = root_with_existing_realm().await;
    let previous = existing_realm_files(&root.path().join("state"));
    let mut cli = Cli::spawn_configured_in(root, None, &provider.origin, Some(&prefix), true, None, None);
    provider.with_profile_provider(cli.shown("Choose a realm number, 'new', or /quit: ", 0)).await;
    assert!(cli.transcript.contains(&format!("1. {EXISTING_REALM} (")));
    cli.send("new\n");
    provider.with_profile_provider(cli.shown("New realm: ", 0)).await;
    cli.send(&format!("{REALM}\n"));
    provider.with_profile_provider(cli.approve_setup_helper()).await;
    provider.with_profile_provider(cli.enter_conversation()).await;
    assert!(provider.requests.lock().unwrap().is_empty());
    let setup_start = cli.transcript.len();
    cli.send(&format!("{SETUP_PROMPT}\n"));
    provider.with_profile_provider(cli.shown("Type 'approve ", setup_start)).await;
    assert!(cli.transcript[setup_start..].contains("service_software_install"));
    let software_id = cli.transcript[setup_start..].split("Type 'approve ").last().unwrap()
        .split('\'').next().unwrap().to_owned();
    let software_end = cli.transcript.len();
    cli.send(&format!("approve {software_id}\n"));
    provider.with_profile_provider(cli.shown(ACCESS_REVIEW, software_end)).await;
    provider.with_profile_provider(cli.shown("Type 'approve ", software_end)).await;
    let access_id = cli.transcript[software_end..].split("Type 'approve ").last().unwrap()
        .split('\'').next().unwrap().to_owned();
    assert_ne!(software_id, access_id);
    for expected in ["public_service_access", "rkat-agent", REALM, OPERATION] {
        assert!(cli.transcript[software_end..].contains(expected), "protected access review omitted {expected}");
    }
    let observed_helpers = observed_setup_helpers(&mut cli).await;
    cli.send(&format!("approve {access_id}\n"));
    provider.with_profile_provider(cli.shown(SETUP_DONE, software_end)).await;
    let setup_session = cli.reviewed_session();
    // The model handles the first process-owned notice and must acquire the
    // second package through a fresh helper, using the same original prompt.
    // The old access prompt is before this boundary; wait for the next software
    // prompt explicitly rather than matching retained terminal history.
    let second_start = cli.transcript.find(SETUP_DONE).unwrap() + SETUP_DONE.len();
    provider.with_profile_provider(cli.shown("Type 'approve ", second_start)).await;
    assert!(cli.transcript[second_start..].contains("service_software_install"));
    assert_observed_helpers_gone(&observed_helpers).await;
    let second_software_id = cli.transcript[second_start..].split("Type 'approve ").last().unwrap()
        .split('\'').next().unwrap().to_owned();
    assert!(second_software_id != software_id && second_software_id != access_id);
    let second_software_end = cli.transcript.len();
    cli.send(&format!("approve {second_software_id}\n"));
    provider.with_profile_provider(cli.shown(ACCESS_REVIEW, second_software_end)).await;
    provider.with_profile_provider(cli.shown("Type 'approve ", second_software_end)).await;
    let second_access_id = cli.transcript[second_software_end..].split("Type 'approve ").last().unwrap()
        .split('\'').next().unwrap().to_owned();
    for prior in [&software_id, &access_id, &second_software_id] { assert_ne!(&second_access_id, prior); }
    for expected in ["public_service_access", "rkat-agent", REALM, SECOND_OPERATION] {
        assert!(cli.transcript[second_software_end..].contains(expected), "second protected review omitted {expected}");
    }
    let second_helpers = observed_setup_helpers(&mut cli).await;
    cli.send(&format!("approve {second_access_id}\n"));
    provider.with_profile_provider(cli.shown(SECOND_DONE, second_software_end)).await;
    provider.with_profile_provider(cli.shown(CONTINUED, second_software_end)).await;
    provider.with_profile_provider(cli.shown("You: ", second_software_end)).await;
    assert_eq!(cli.reviewed_session(), setup_session);
    assert_observed_helpers_gone(&second_helpers).await;
    let setup_body = provider.requests.lock().unwrap().iter()
        .find(|request| optional_result(&request.body, "second-review-public-access").is_some()).unwrap().body.clone();
    let descriptors = [reviewed_descriptor(&setup_body, "", "hackernews", OPERATION),
        reviewed_descriptor(&setup_body, SECOND_PREFIX, "wikipedia", SECOND_OPERATION)];
    assert_ne!(descriptors[0].name, descriptors[1].name);
    cli.send("/quit\n");
    assert!(provider.with_profile_provider(cli.finish_with_installations(&descriptors))
        .await.success(), "{}", cli.transcript);
    assert_eq!(cli.transcript.matches("native access is selected for agent rkat-agent").count(), 2);
    assert!(!cli.transcript.contains("Previous grant cleanup is incomplete"));
    assert_eq!(cli.transcript.matches("Type 'approve ").count(), 6,
        "two initial setup reviews, then two independent reviews for each service");
    assert_eq!(existing_realm_files(&cli.state), previous);
    let config = fs::read(cli.state.join(REALM).join("config.toml")).unwrap();
    let setup_calls: Vec<_> = setup_body["messages"].as_array().unwrap().iter()
        .flat_map(|message| message["tool_calls"].as_array().into_iter().flatten()).collect();
    for name in ["toolkit_prepare_services", "setup_prepare", "setup_check", "setup_install", "toolkit_enable_public_service"] {
        assert_eq!(setup_calls.iter().filter(|call| call["function"]["name"] == name).count(), 2,
            "exactly one real {name} per package, no replay of the prior effect");
    }
    assert_eq!(setup_calls.iter().filter(|call| call["function"]["name"] == "setup_reconcile").count(), 3,
        "both current jobs plus canonical readback of the first job from the second helper");
    assert!(!setup_calls.iter().any(|call| matches!(call["function"]["name"].as_str(), Some(OPERATION | SECOND_OPERATION))));
    let setup_requests = provider.requests.lock().unwrap().len();
    assert_observed_helpers_gone(&observed_helpers).await;
    assert_observed_helpers_gone(&second_helpers).await;
    let output = cli.start_fresh_rkat(&binary, &expected_sha256);
    assert!(provider.with_profile_provider(cli.finish()).await.success(), "{}", cli.transcript);
    provider.finish().await;
    assert_eq!(fs::read(cli.state.join(REALM).join("config.toml")).unwrap(), config);
    assert_eq!(existing_realm_files(&cli.state), previous);
    let requests = provider.requests.lock().unwrap();
    assert_eq!(requests.len(), setup_requests + 4, "one search, one load and one service call with their actual results, no retry");
    let final_body = &requests.last().unwrap().body;
    let service_result = final_body["messages"].as_array().unwrap().iter()
        .find(|message| message["role"] == "tool" && message["tool_call_id"] == STORY_CALL).unwrap();
    let stories = public_stories(service_result["content"].as_str().unwrap()).unwrap();
    #[derive(serde::Deserialize)]
    struct RunResult {
        session_id: meerkat_core::SessionId,
        session_ref: String,
        text: String,
        tool_calls: u32,
    }
    assert!(fs::metadata(&output).unwrap().len() <= 1024 * 1024);
    let result: RunResult = serde_json::from_slice(&fs::read(output).unwrap()).unwrap();
    assert_ne!(result.session_id, setup_session);
    assert_eq!(result.session_ref, format!("{REALM}:{}", result.session_id));
    assert_eq!(result.tool_calls, 3, "two catalog operations and exactly one service call");
    assert_eq!(result.text, story_answer(&stories));
    let retained_root = keep_realm.then(|| cli._root.take().unwrap().keep());
    println!("{}", json!({
        "event":"connect_cli_fresh_rkat_three_stories",
        "scope":"scripted_loopback_model_actual_cli_and_fresh_ordinary_rkat_real_public_https",
        "realm":REALM, "retained_root":retained_root, "setup_session_id":setup_session,
        "fresh_session_id":result.session_id, "session_ref":result.session_ref,
        "cli_sha256":file_digest(&prefix.join("bin/meerkat-connect")),
        "rkat_sha256":expected_sha256, "stories":stories, "answer":result.text,
        "business_tool_calls":result.tool_calls, "setup_exited_before_fresh_rkat":true,
        "fresh_rkat_exited_successfully":true, "realm_configuration_unchanged":true,
        "observed_setup_helpers_absent":true, "setup_services":["hackernews","wikipedia"],
    }));
}

#[test]
fn public_story_oracle_rejects_error_empty_and_disagreeing_results() {
    let value = json!([{"id":1,"title":"First control"},
        {"id":2,"title":"Second control"},{"id":3,"title":"Third control"}]).to_string();
    let content = format!("{value}\n{}", json!({"value":value}));
    assert_eq!(public_stories(&content).unwrap().iter().map(|story| story.id).collect::<Vec<_>>(), vec![1,2,3]);
    for invalid in [json!([{"id":1,"title":"Only one"}]),
        json!([{"id":1,"title":"First"},{"id":1,"title":"Repeated"},{"id":3,"title":"Third"}])] {
        let text = invalid.to_string();
        assert!(public_stories(&format!("{text}\n{}", json!({"value":text}))).is_err());
    }
    for content in [
        "Upstream operation failed\n{\"error\":{\"entered\":true}}".to_owned(),
        format!("[]\n{}", json!({"value":"[]"})),
        format!("{value}\n{}", json!({"value":"[]"})),
        format!("{value}\n{}", json!({"value":value,"error":{"entered":true}})),
        format!("{value}\n{}\n{{}}", json!({"value":value})),
    ] { assert!(public_stories(&content).is_err()); }
}


#[test]
fn public_finalization_notice_parser_preserves_ordered_native_outcomes() {
    let first = json!({"kind":"connect_finalization_observation","realm":REALM,
        "source_job":"first-job","server":"hackernews","consumer_agent":"rkat-agent",
        "facts":{"persistence":"saved","grant":{"state":"selected"}},
        "service_readiness":"not_observed","automatic_reentry_blocked":false});
    let second = json!({"kind":"connect_finalization_observation","realm":REALM,
        "source_job":"second-job","server":"wikipedia","consumer_agent":"rkat-agent",
        "facts":{"persistence":"saved","grant":{"state":"selected"}},
        "service_readiness":"not_observed","automatic_reentry_blocked":false});
    let first_text = finalization_projection(&first);
    let second_text = finalization_projection(&second);
    let messages = vec![json!({"role":"user","content":SETUP_PROMPT}),
        json!({"role":"user","content":first_text}),
        json!({"role":"assistant","content":SETUP_DONE}),
        json!({"role":"user","content":second_text})];
    assert_eq!(finalization_notices(&messages[..1]).unwrap(), Vec::<Value>::new());
    assert_eq!(finalization_notices(&messages[..3]).unwrap(), vec![first.clone()]);
    assert_eq!(finalization_notices(&messages).unwrap(), vec![first, second]);
}

#[test]
fn public_finalization_notice_parser_refuses_partial_altered_and_replayed_projections() {
    let outcome = json!({"kind":"connect_finalization_observation","source_job":"exact-job",
        "facts":{"persistence":"saved"}});
    let text = finalization_projection(&outcome);
    let message = |content: String| json!({"role":"user","content":content});
    for invalid in [
        outcome.to_string(),
        text.replacen("saved", "changed", 1),
        format!("{text}\nextra"),
        format!("{text}\n{text}"),
        text.replace("meerkat-connect-finalization", "foreign-source"),
    ] { assert!(finalization_notices(&[message(invalid)]).is_err()); }
    assert!(finalization_notices(&[message(text.clone()), message(text)]).is_err());
}

#[test]
fn public_finalization_notice_parser_ignores_malformed_unrelated_and_non_user_text() {
    use meerkat_core::types::{SystemNoticeBlock, SystemNoticeKind, SystemNoticeMessage};
    let foreign = json!({"kind":"another_native_event","value":"unrelated"}).to_string();
    let foreign_text = SystemNoticeMessage::with_blocks(SystemNoticeKind::ExternalEvent,
        Some(foreign.clone()), vec![SystemNoticeBlock::ExternalEvent {
            source:"another-native-owner".into(), event_type:"another_native_event".into(),
            summary:Some(foreign.clone()), body:Some(foreign.clone()),
            payload:Some(json!({"body":foreign})), content:Vec::new(),
        }]).model_projection_text();
    let outcome = json!({"kind":"connect_finalization_observation","source_job":"exact-job"});
    let text = finalization_projection(&outcome);
    let messages = vec![
        json!({"role":"user","content":"{\"kind\":\"connect_finalization_observation\","}),
        json!({"role":"user","content":foreign_text}),
        json!({"role":"user","content":format!("External event via meerkat-connect-finalization: {outcome}")}),
        json!({"role":"assistant","content":text}),
        json!({"role":"tool","tool_call_id":"unrelated","content":text}),
    ];
    assert!(finalization_notices(&messages).unwrap().is_empty());
}

#[test]
fn fresh_public_request_accepts_only_one_business_input_and_canonical_boundary_notice() {
    let notice = tool_scope_projection(true, false, 0);
    assert_eq!(notice, concat!(
        "Tool configuration changed at turn boundary: boundary_applied(base_changed=true,visible_changed=false,revision=0)\n",
        "Tool configuration changed: boundary_applied(base_changed=true,visible_changed=false,revision=0)"));
    let user = |content: &str| json!({"role":"user","content":content});
    let base = vec![json!({"role":"system","content":"Isolated fresh session"}), user(RUN_PROMPT)];
    let request = |messages: Vec<Value>, loaded: bool| Request {
        method: "POST".into(), path: "/v1/chat/completions".into(),
        headers: BTreeMap::from([("authorization".into(), format!("Bearer {TOKEN}"))]),
        body: json!({"model":"cli-fixture-remote","messages":messages,
            "tools":if loaded { vec![json!({"function":{"name":OPERATION}})] } else {
                vec![json!({"function":{"name":"tool_catalog_search"}}),
                    json!({"function":{"name":"tool_catalog_load"}})] }}),
    };
    let append = |messages: &mut Vec<Value>, index: usize, content: String| {
        let (id, name, arguments) = fresh_call(index);
        messages.push(json!({"role":"assistant","content":null,"tool_calls":[
            {"id":id,"type":"function","function":{"name":name,"arguments":arguments.to_string()}}]}));
        messages.push(json!({"role":"tool","tool_call_id":id,"content":content}));
    };
    // Match selected native serialization, including omitted empty search fields
    // and the typed load resolution's default false accepted_noop field.
    let found = json!({"catalog_exact":true,"total_matches":1,"results":[
        {"name":OPERATION,"description":"Public stories","currently_callable":false,
            "visibility_status":"deferred"}]});
    let loaded = json!({"catalog_exact":true,"accepted_names":[OPERATION],"noop_names":[],
        "resolutions":[{"name":OPERATION,"accepted":true}]});
    let value = json!([{"id":1,"title":"First control"},
        {"id":2,"title":"Second control"},{"id":3,"title":"Third control"}]).to_string();
    let story = format!("{value}\n{}", json!({"value":value}));
    for (fields, addition) in [(None, false), (None, true), (Some((true, false, 0)), true),
        (Some((false, true, 1)), true), (Some((true, true, u64::MAX)), true)] {
        let mut messages = base.clone();
        if let Some((base, visible, revision)) = fields {
            messages.push(user(&tool_scope_projection(base, visible, revision)));
        }
        if addition { messages.push(user(&fresh_deferred_addition())); }
        for (index, result) in [found.to_string(), loaded.to_string(), story.clone()].into_iter().enumerate() {
            let (id, name, arguments) = fresh_call(index);
            assert_eq!(response(&request(messages.clone(), index == 2), index),
                deferred_call_response(id, name, arguments));
            append(&mut messages, index, result);
            if index == 1 && addition {
                messages.push(user(&tool_scope_projection(false, true, 2)));
                messages.push(user(&deferred_catalog_projection(&[vec![], vec![OPERATION], vec![]])));
            }
        }
        assert_eq!(response(&request(messages, true), 3), completed("Hacker News story 1: First control\nHacker News story 2: Second control\nHacker News story 3: Third control"));
    }
    let mut noop = loaded.clone();
    noop["accepted_names"] = json!([]);
    noop["noop_names"] = json!([OPERATION]);
    noop["resolutions"][0]["accepted_noop"] = json!(true);
    let mut already_loaded = found.clone();
    already_loaded["results"][0]["currently_callable"] = json!(true);
    already_loaded["results"][0]["visibility_status"] = json!("loaded");
    let mut messages = base.clone();
    append(&mut messages, 0, already_loaded.to_string());
    append(&mut messages, 1, noop.to_string());
    assert_eq!(response(&request(messages, true), 2),
        deferred_call_response(STORY_CALL, OPERATION, json!({"num_stories":3})));

    let finalization = finalization_projection(&json!({"kind":"connect_finalization_observation",
        "facts":{"persistence":"saved","grant":{"state":"selected"}}}));
    for extra in [
        vec![user(RUN_PROMPT)], vec![user(SETUP_PROMPT)], vec![user(&finalization)],
        vec![user("unrelated prior user instruction")], vec![user(&notice), user(&notice)],
        vec![user(&tool_scope_projection(false, false, 0))],
        vec![user(&notice.replace("base_changed=true", "base_changed=True"))],
        vec![user(&notice.replace("revision=0", "revision=00"))],
        vec![user(&notice.replace("revision=0", "revision=18446744073709551616"))],
        vec![user(&notice.replacen("revision=0", "revision=1", 1))],
        vec![user(notice.lines().next().unwrap())], vec![user(&format!("{notice}\nextra"))],
        vec![user(&fresh_deferred_addition()), user(&fresh_deferred_addition())],
        vec![user(&fresh_deferred_addition().replace("setup_state", "setup_install"))],
        vec![user(&deferred_catalog_projection(&[vec![OPERATION], vec![], vec![]]))],
        vec![user(&deferred_catalog_projection(&[vec![], vec![OPERATION], vec![]]))],
        vec![user(&deferred_catalog_projection(&[vec![], vec![], vec!["connecting"]]))],
        vec![json!({"role":"user","content":[{"type":"text","text":RUN_PROMPT}]})],
        vec![json!({"role":"assistant","content":SETUP_DONE})],
        vec![json!({"role":"tool","tool_call_id":"old-setup-call","content":"old result"})],
    ] {
        let mut messages = base.clone();
        messages.extend(extra);
        assert!(std::panic::catch_unwind(|| response(&request(messages, false), 0)).is_err());
    }
    for (field, replacement) in [("catalog_exact", json!(false)),
        ("pending_sources", json!(["connecting"])), ("pending_sources", json!(null)),
        ("empty_result_status", json!("pending_source")),
        ("results", json!([])), ("results", json!([found["results"][0],found["results"][0]])),
        ("results", json!([{"name":SECOND_OPERATION,"visibility_status":"deferred"}])),
        ("results", json!([{"name":OPERATION,"visibility_status":"blocked_by_filter"}]))] {
        let mut rejected = found.clone();
        rejected[field] = replacement;
        let mut messages = base.clone();
        append(&mut messages, 0, rejected.to_string());
        assert!(std::panic::catch_unwind(|| response(&request(messages, false), 1)).is_err());
    }
    for index in 0..7 {
        let mut rejected = loaded.clone();
        match index {
            0 => rejected["catalog_exact"] = json!(false),
            1 => rejected["accepted_names"] = json!([OPERATION, SECOND_OPERATION]),
            2 => rejected["resolutions"][0]["accepted"] = json!(false),
            3 => rejected["resolutions"][0]["rejected_reason"] = json!("temporarily_unavailable"),
            4 => rejected["resolutions"][0]["accepted_noop"] = json!(true),
            5 => rejected["resolutions"][0]["accepted_noop"] = json!("false"),
            _ => rejected["resolutions"][0]["name"] = json!(SECOND_OPERATION),
        }
        let mut messages = base.clone();
        append(&mut messages, 0, found.to_string());
        append(&mut messages, 1, rejected.to_string());
        assert!(std::panic::catch_unwind(|| response(&request(messages, true), 2)).is_err());
    }
    let mut complete = base.clone();
    append(&mut complete, 0, found.to_string());
    append(&mut complete, 1, loaded.to_string());
    assert!(std::panic::catch_unwind(|| response(&request(complete.clone(), false), 2)).is_err());
    for index in 0..6 {
        let mut rejected = complete.clone();
        match index {
            0 => { rejected.remove(2); }, // Result without its actual preceding call.
            1 => rejected[2]["tool_calls"][0]["function"]["arguments"] = json!("{\"query\":\"setup_install\"}"),
            2 => rejected[3]["tool_call_id"] = json!(LOAD_CALL),
            3 => { append(&mut rejected, 0, found.to_string()); }, // Replayed metadata call.
            4 => { rejected.extend(complete[4..].iter().cloned()); }, // Replayed load.
            _ => { append(&mut rejected, 2, story.clone()); append(&mut rejected, 2, story.clone()); },
        }
        assert!(std::panic::catch_unwind(|| response(&request(rejected, true), 2)).is_err());
    }
    let mut checked = request(base.clone(), false);
    checked.body["tools"].as_array_mut().unwrap().push(json!({"function":{"name":"setup_install"}}));
    assert!(std::panic::catch_unwind(|| response(&checked, 0)).is_err());
    let private = "fixture-private-content-must-not-be-printed";
    let mut rejected = base;
    rejected.extend((0..20).map(|_| user(private)));
    let failure = std::panic::catch_unwind(|| response(&request(rejected.clone(), false), 0)).unwrap_err();
    let text = failure.downcast_ref::<String>().unwrap();
    assert!(!text.contains(private) && !text.contains(TOKEN));
    let shape = fresh_input_shape(&rejected);
    assert_eq!(shape["message_count"], 22);
    assert_eq!(shape["messages"].as_array().unwrap().len(), 16);
    assert!(text.contains(shape["messages"][2]["content_json_sha256"].as_str().unwrap()));
}

#[test]
fn fresh_input_failure_diagnostic_excludes_arbitrary_payloads_and_bounds_parts() {
    let private = "fixture-private-message-type-key-url-arguments-must-not-be-printed";
    let warning = format!("Tool scope apply failed ({private})");
    let deferred = format!("Deferred catalog changed at turn boundary: {private}");
    let mut parts = vec![
        json!({"type":"text","text":deferred}),
        json!({"type":private,"text":private,"arguments":private}),
        json!({"type":"image_url","image_url":{"url":private}}),
        json!(private),
        json!({"text":{(private):private}}),
        json!(null), json!(true), json!(3),
    ];
    parts.extend((0..4).map(|_| json!({"type":"text","text":private})));
    let messages = vec![
        json!({"role":"user","content":RUN_PROMPT}),
        json!({"role":"user","content":warning,"headers":{(private):private}}),
        json!({"role":"user","content":parts,"tool_calls":[{"arguments":private}]}),
        json!({"role":private,"content":{(private):private}}),
    ];
    let failure = std::panic::catch_unwind(|| assert_fresh_business_input(&messages)).unwrap_err();
    let diagnostic = failure.downcast_ref::<String>().unwrap();
    for excluded in [private, TOKEN, RUN_PROMPT, warning.as_str(), deferred.as_str()] {
        assert!(!diagnostic.contains(excluded), "failure diagnostic exposed message content");
    }
    let shape = fresh_input_shape(&messages);
    assert!(diagnostic.contains(&shape.to_string()), "actual failure must retain only the bounded shape");
    let scalar = &shape["messages"][1]["content_shape"];
    assert_eq!(scalar["json_kind"], "string");
    assert_eq!(scalar["text"]["bytes"], warning.len());
    assert_eq!(scalar["text"]["sha256"], format!("{:x}", Sha256::digest(warning.as_bytes())));
    assert_eq!(scalar["text"]["display_prefix_hint"], "tool_scope_warning");
    let array = &shape["messages"][2]["content_shape"];
    assert_eq!(array["json_kind"], "array");
    assert_eq!(array["part_count"], 12);
    assert_eq!(array["parts"].as_array().unwrap().len(), 8);
    assert_eq!(array["parts"][0]["text"]["display_prefix_hint"], "deferred_catalog");
    assert_eq!(array["parts"][1]["part_type"], "other");
    assert_eq!(array["parts"][1]["text"]["display_prefix_hint"], "unknown");
    assert_eq!(array["parts"][2]["part_type"], "image_url");
    assert!(array["parts"][2]["text"].is_null());
    assert_eq!(array["parts"][3]["json_kind"], "string");
    assert_eq!(array["parts"][3]["text"]["sha256"], format!("{:x}", Sha256::digest(private.as_bytes())));
    assert_eq!(array["parts"][4]["text_kind"], "object");
    assert!(array["parts"][4]["text"].is_null());
    assert_eq!(shape["messages"][3]["role"], "other");
    assert_eq!(shape["messages"][3]["content_shape"]["json_kind"], "object");

    // A complete bounded display is useful diagnostic data but remains refused
    // by the unchanged fresh-input oracle. Do not treat fragments as tool IDs.
    let groups = [vec!["native_one", "native_two"], vec!["retired_one"], vec!["source_one"]];
    let notice = deferred_catalog_projection(&groups);
    let rejected = vec![json!({"role":"user","content":RUN_PROMPT}),
        json!({"role":"user","content":notice})];
    let failure = std::panic::catch_unwind(|| assert_fresh_business_input(&rejected)).unwrap_err();
    let diagnostic = failure.downcast_ref::<String>().unwrap();
    let display = deferred_catalog_display_fragments(&notice).unwrap();
    assert_eq!(display, json!({"added_hidden_count":2,"removed_hidden_count":1,
        "pending_source_count":1,"added_display_fragments":["native_one","native_two"],
        "removed_display_fragments":["retired_one"],"pending_display_fragments":["source_one"]}));
    assert_eq!(fresh_text_shape(&notice)["deferred_catalog_display"], display);
    assert!(diagnostic.contains(&display.to_string()));
    assert!(!diagnostic.contains(TOKEN) && !diagnostic.contains(RUN_PROMPT));

    let many: Vec<_> = (0..17).map(|index| format!("name_{index:02}")).collect();
    let oversized = "x".repeat(129);
    let aggregate: Vec<_> = (0..9).map(|index| format!("name_{index}_{}", "x".repeat(113))).collect();
    for invalid in [
        notice.replace("deferred catalog updated", "deferred catalog changed"),
        notice.replace("deferred tools now available to call: ", "deferred tools removed: "),
        notice.replace("sources still connecting: ", "sources ready: "),
        notice.replace("Tool configuration changed: ", "Tool configuration pending: "),
        format!("{notice}\n{private}"),
        format!("{}; {private}\n{}", notice.lines().next().unwrap(), notice.lines().nth(1).unwrap()),
        notice.lines().next().unwrap().to_owned(),
        notice.replace("native_one, native_two", "native_one,native_two"),
        deferred_catalog_projection(&[vec!["native_two", "native_one"], vec![], vec![]]),
        deferred_catalog_projection(&[vec!["native_one", "native_one"], vec![], vec![]]),
        deferred_catalog_projection(&[vec!["native_one"], vec!["native_one"], vec![]]),
        deferred_catalog_projection(&[vec!["native_one; other"], vec![], vec![]]),
        deferred_catalog_projection(&[vec!["native_one,other"], vec![], vec![]]),
        deferred_catalog_projection(&[many.iter().map(String::as_str).collect(), vec![], vec![]]),
        deferred_catalog_projection(&[vec![oversized.as_str()], vec![], vec![]]),
        deferred_catalog_projection(&[aggregate.iter().map(String::as_str).collect(), vec![], vec![]]),
        format!("{notice}{}", "x".repeat(4096)),
    ] {
        assert_ne!(invalid, notice, "negative control must change the native projection");
        let shape = fresh_text_shape(&invalid);
        assert!(shape["deferred_catalog_display"].is_null());
        let rejected = vec![json!({"role":"user","content":RUN_PROMPT}),
            json!({"role":"user","content":invalid})];
        let failure = std::panic::catch_unwind(|| assert_fresh_business_input(&rejected)).unwrap_err();
        let diagnostic = failure.downcast_ref::<String>().unwrap();
        assert!(!diagnostic.contains(private) && !diagnostic.contains(TOKEN)
            && !diagnostic.contains("native_one"), "noncanonical display leaked its fragments");
    }
}
