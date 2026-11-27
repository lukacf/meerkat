//! Explicit installed-product composition acceptance. Run the compiled test
//! executable from the assembled product's bin directory, outside the checkout,
//! with PATH naming an empty directory. This is not terminal/TTY acceptance.

use std::{
    collections::BTreeSet,
    error::Error,
    fs::OpenOptions,
    io::Write,
    path::PathBuf,
    pin::Pin,
    sync::{Arc, Mutex},
    time::Duration,
};

use async_trait::async_trait;
use futures_util::Stream;
use meerkat::{Config, LlmClient, LlmDoneOutcome, LlmError, LlmEvent, LlmRequest, StopReason};
use meerkat_connect::{
    ConnectSession, RealmOptions, SessionOptions, resources::InstalledResources,
};
use meerkat_core::{
    Message, Provider, SessionBuildOptions, ToolAccessPolicy, ToolResult, TurnUsage, Usage,
};
use meerkat_runtime::completion::CompletionOutcome;
use serde_json::{Value, json};

type Result<T = ()> = std::result::Result<T, Box<dyn Error + Send + Sync>>;

const SETUP_TOOLS: &[&str] = &[
    "setup_catalog",
    "setup_catalog_entry",
    "setup_prepare",
    "setup_state",
    "setup_help",
    "setup_install",
];
const PREPARATION_DENIED: &[&str] = &["setup_install"];
const COMPOSITION_DENIED: &[&str] = &["setup_catalog_entry", "setup_install"];

struct Command {
    id: String,
    name: String,
    args: Value,
    issued: bool,
    result: Option<ToolResult>,
}

struct Probe {
    inner: meerkat_client::TestClient,
    command: Mutex<Option<Command>>,
    trace: PathBuf,
    stall: std::sync::atomic::AtomicBool,
    started: tokio::sync::Notify,
}

impl Probe {
    fn record(&self, value: Value) {
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.trace)
            .expect("retain native dispatch trace");
        writeln!(file, "{value}").expect("write native dispatch trace");
    }
}

#[async_trait]
impl LlmClient for Probe {
    fn project_replay_messages(
        &self,
        messages: &[Message],
    ) -> std::result::Result<Vec<Message>, LlmError> {
        self.inner.project_replay_messages(messages)
    }

    fn stream<'a>(
        &'a self,
        request: &'a LlmRequest,
    ) -> Pin<Box<dyn Stream<Item = std::result::Result<LlmEvent, LlmError>> + Send + 'a>> {
        if self.stall.load(std::sync::atomic::Ordering::SeqCst) {
            self.started.notify_one();
            return Box::pin(futures_util::stream::pending());
        }
        let mut guard = self.command.lock().expect("test command");
        let command = guard.as_mut().expect("one test-selected native call");
        if command.issued {
            for message in &request.messages {
                if let Message::ToolResults { results, .. } = message
                    && let Some(result) = results
                        .iter()
                        .find(|result| result.tool_use_id == command.id)
                    && command.result.is_none()
                {
                    self.record(
                        json!({"kind":"native_tool_result","name":command.name,"result":result}),
                    );
                    command.result = Some(result.clone());
                }
            }
            drop(guard);
            return self.inner.stream(request);
        }
        if !request.tools.iter().any(|tool| tool.name == command.name) {
            self.record(
                json!({"kind":"test_call_refused_not_visible","name":command.name,
                "visible_tools":request.tools.iter().map(|tool| &tool.name).collect::<Vec<_>>() }),
            );
            return Box::pin(futures_util::stream::iter([Err(
                LlmError::InvalidRequest {
                    message: format!("Acceptance refuses to emit unoffered tool {}", command.name),
                },
            )]));
        }
        command.issued = true;
        self.record(json!({"kind":"native_model_call","id":command.id,"name":command.name,
            "arguments":command.args,"visible_tools":request.tools.iter().map(|tool| &tool.name).collect::<Vec<_>>() }));
        let events = vec![
            Ok(LlmEvent::ToolCallComplete {
                id: command.id.clone(),
                name: command.name.clone(),
                args: command.args.clone(),
                meta: None,
            }),
            Ok(LlmEvent::UsageUpdate {
                usage: TurnUsage::host_declared(Provider::OpenAI, &request.model, Usage::default()),
            }),
            Ok(LlmEvent::Done {
                outcome: LlmDoneOutcome::success(StopReason::ToolUse),
            }),
        ];
        Box::pin(futures_util::stream::iter(events))
    }

    fn provider(&self) -> Provider {
        Provider::OpenAI
    }
    async fn health_check(&self) -> std::result::Result<(), LlmError> {
        Ok(())
    }
}

fn require(condition: bool, message: &str) -> Result {
    if condition {
        Ok(())
    } else {
        Err(std::io::Error::other(message).into())
    }
}

async fn call(
    session: &ConnectSession,
    probe: &Probe,
    name: &str,
    args: Value,
) -> Result<ToolResult> {
    call_before(session, probe, name, args, None).await
}

async fn call_before(
    session: &ConnectSession,
    probe: &Probe,
    name: &str,
    args: Value,
    deadline: Option<tokio::time::Instant>,
) -> Result<ToolResult> {
    let id = format!("acceptance-{}", meerkat_core::SessionId::new());
    *probe.command.lock().expect("test command") = Some(Command {
        id,
        name: name.into(),
        args,
        issued: false,
        result: None,
    });
    let (input, completion) = session
        .submit(format!(
            "Acceptance: invoke {name} with the supplied exact test arguments."
        ))
        .await?;
    let wait = completion.wait();
    tokio::pin!(wait);
    let mut timed_out = false;
    let outcome = if let Some(deadline) = deadline {
        tokio::select! {
            result = &mut wait => result?,
            _ = tokio::time::sleep_until(deadline) => {
                timed_out = true;
                session.cancel(&input).await?;
                wait.await?
            }
        }
    } else {
        wait.await?
    };
    probe.record(json!({"kind":"native_completion","name":name,"outcome":format!("{outcome:?}")}));
    require(
        !timed_out,
        "catalog readiness deadline expired; exact native input cancellation was joined",
    )?;
    require(
        matches!(outcome, CompletionOutcome::Completed(_)),
        "native turn did not complete; see retained trace",
    )?;
    probe
        .command
        .lock()
        .expect("test command")
        .as_mut()
        .and_then(|command| command.result.take())
        .ok_or_else(|| std::io::Error::other("native model did not receive a tool result").into())
}

fn value(result: ToolResult) -> Result<Value> {
    require(
        !result.is_error,
        &format!("real tool failed: {}", result.text_content()),
    )?;
    Ok(serde_json::from_str(&result.text_content())?)
}

// What the model reads from the catalog search does not prove a policy
// ceiling: under one, a denied tool has no row. The ceiling is therefore
// established in two halves:
//
// 1. Host side, never from anything the model is shown: the launch tool
//    policy the packaged setup composed into the session's own options
//    (`SessionOptions::build.tool_access_policy`, the caller's policy
//    conjoined with the preparation ceiling) denies exactly the expected
//    setup names (`require_setup_ceiling`).
// 2. Caller side: every permitted setup name is found and loads on that
//    session (the search works and the source is connected), and each denied
//    name is absent from the search (`setup_catalog_ready`). Tool load and
//    call behavior is unchanged (`setup_load_accepted`).
//
// The positive control for the caller's ceiling is the same packaged setup
// without it: the preparation-only session of `active_worker` finds and loads
// `setup_catalog_entry`, which the caller's deny list hides here. No packaged
// session lifts the preparation ceiling over `setup_install` (installation
// stays refused by design), so that name has the host-side half only.

/// The setup names the session's own launch tool policy does not permit: a
/// host-side read of the options the packaged setup composed for the session.
fn setup_names_denied_by_launch_policy(options: &SessionOptions) -> Result<BTreeSet<&'static str>> {
    let policy = options
        .build
        .tool_access_policy
        .clone()
        .ok_or("the packaged setup session carries no launch tool policy")?;
    let policy = meerkat_core::ToolExecutionPolicy::resolve(policy)?;
    Ok(SETUP_TOOLS
        .iter()
        .copied()
        .filter(|name| !policy.permits(name))
        .collect())
}

fn require_setup_ceiling(options: &SessionOptions, denied: &[&str]) -> Result {
    require(
        setup_names_denied_by_launch_policy(options)? == denied.iter().copied().collect(),
        "the session's launch tool policy does not deny exactly the expected setup names",
    )
}

// These are assertions over native observations, not a second readiness owner.
// Exact catalog means exact current contents; it does not mean a pending source
// has connected. Only that explicit pending observation permits another search.
fn setup_catalog_ready(search: &Value, denied: &[&str]) -> Result<bool> {
    require(
        search["catalog_exact"] == true,
        "native tool catalog is not exact",
    )?;
    if let Some(pending) = search.get("pending_sources") {
        let pending = pending.as_array().ok_or("invalid native pending sources")?;
        require(
            pending.len() <= 1 && pending.iter().all(|name| name == "toolkit-setup"),
            "an unselected source is pending",
        )?;
        if !pending.is_empty() {
            return Ok(false);
        }
    }
    require(
        search["empty_result_status"].is_null(),
        "catalog has an unresolved empty-result status",
    )?;
    let rows = search["results"]
        .as_array()
        .ok_or("native catalog results missing")?;
    for name in SETUP_TOOLS {
        let matches = rows
            .iter()
            .filter(|row| row["name"] == *name)
            .collect::<Vec<_>>();
        if denied.contains(name) {
            // The catalog offers a session only what its tool policy allows:
            // a denied name is not a row, whatever its status. On its own
            // this is not proof of a ceiling (see `require_setup_ceiling`).
            require(
                matches.is_empty(),
                &format!("catalog offered the policy-denied tool {name}"),
            )?;
            continue;
        }
        require(
            matches.len() == 1,
            &format!("ready catalog does not contain exact tool {name}"),
        )?;
        let row = matches[0];
        require(
            (row["visibility_status"] == "deferred" && row["currently_callable"] == false)
                || (row["visibility_status"] == "loaded" && row["currently_callable"] == true),
            &format!("catalog tool {name} is not loadable or loaded"),
        )?;
    }
    Ok(true)
}

fn observed_names(value: &Value) -> Result<BTreeSet<&str>> {
    let names = value.as_array().ok_or("native load name list missing")?;
    let result = names
        .iter()
        .map(|name| name.as_str().ok_or("invalid native load name"))
        .collect::<std::result::Result<BTreeSet<_>, _>>()?;
    require(
        result.len() == names.len(),
        "native load names are duplicated",
    )?;
    Ok(result)
}

fn setup_load_accepted(load: &Value, denied: &[&str]) -> Result {
    require(
        load["catalog_exact"] == true,
        "load did not use an exact catalog",
    )?;
    let accepted = observed_names(&load["accepted_names"])?;
    let noops = observed_names(&load["noop_names"])?;
    let expected = SETUP_TOOLS
        .iter()
        .copied()
        .filter(|name| !denied.contains(name))
        .collect::<BTreeSet<_>>();
    require(
        accepted.is_disjoint(&noops)
            && accepted.union(&noops).copied().collect::<BTreeSet<_>>() == expected,
        "native load did not accept exactly the permitted setup names",
    )?;
    let resolutions = load["resolutions"]
        .as_array()
        .ok_or("native load resolutions missing")?;
    require(
        resolutions.len() == SETUP_TOOLS.len(),
        "native load resolution count changed",
    )?;
    for name in SETUP_TOOLS {
        let matches = resolutions
            .iter()
            .filter(|row| row["name"] == *name)
            .collect::<Vec<_>>();
        require(
            matches.len() == 1,
            "native load resolution missing or duplicated",
        )?;
        let row = matches[0];
        if denied.contains(name) {
            require(
                row["accepted"] == false
                    && row["rejected_reason"] == "not_filterable"
                    && !row
                        .get("accepted_noop")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                &format!("native load did not preserve the exact policy refusal for {name}"),
            )?;
            continue;
        }
        require(
            row["accepted"] == true
                && row["rejected_reason"].is_null()
                && row
                    .get("accepted_noop")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
                    == noops.contains(name),
            &format!("native load rejected or contradicted {name}"),
        )?;
    }
    Ok(())
}

async fn await_setup_catalog(session: &ConnectSession, probe: &Probe, denied: &[&str]) -> Result {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        require(
            tokio::time::Instant::now() < deadline,
            "selected setup source remained pending past readiness deadline",
        )?;
        let search = value(
            call_before(
                session,
                probe,
                "tool_catalog_search",
                json!({"query":"setup","limit":50}),
                Some(deadline),
            )
            .await?,
        )?;
        if setup_catalog_ready(&search, denied)? {
            return Ok(());
        }
        probe.record(json!({"kind":"selected_setup_catalog_pending","source":"toolkit-setup"}));
        tokio::time::sleep_until(
            (tokio::time::Instant::now() + Duration::from_millis(250)).min(deadline),
        )
        .await;
    }
}

/// The session options the packaged setup composes for a caller policy, built
/// from a preparer handoff that names an executable nothing here launches.
fn composed_setup_options(caller: Option<ToolAccessPolicy>) -> SessionOptions {
    meerkat_connect::setup::PreparedSetupConnection::from_preparer_output(
        br#"{"mcpServers":{"toolkit-setup":{"command":"/unlaunched/toolkit-setup"}}}"#,
    )
    .expect("a structurally valid preparer handoff")
    .preparation_session(SessionOptions {
        model: Default::default(),
        build: SessionBuildOptions {
            tool_access_policy: caller,
            ..Default::default()
        },
    })
    .expect("the packaged setup composes its session options")
}

#[test]
fn setup_ceiling_is_read_from_the_host_side_launch_policy() {
    // Preparation only: installation is the one setup name the ceiling denies.
    let preparation = composed_setup_options(None);
    assert_eq!(
        setup_names_denied_by_launch_policy(&preparation).unwrap(),
        PREPARATION_DENIED.iter().copied().collect()
    );
    require_setup_ceiling(&preparation, PREPARATION_DENIED).unwrap();
    assert!(
        require_setup_ceiling(&preparation, COMPOSITION_DENIED).is_err(),
        "a session without the caller's deny list does not carry the caller's ceiling"
    );

    // The caller's deny list is conjoined with the preparation ceiling.
    let composition = composed_setup_options(Some(ToolAccessPolicy::DenyList(
        ["setup_catalog_entry"].into_iter().collect(),
    )));
    require_setup_ceiling(&composition, COMPOSITION_DENIED).unwrap();
    assert!(
        require_setup_ceiling(&composition, PREPARATION_DENIED).is_err(),
        "the caller's ceiling is part of what the session is built with"
    );

    // Options without a launch policy prove no ceiling at all.
    let mut bare = composed_setup_options(None);
    bare.build.tool_access_policy = None;
    assert!(require_setup_ceiling(&bare, &[]).is_err());
}

/// Both halves for the name the caller's ceiling hides: without the ceiling
/// the same catalog and load observations must hold the name, and under the
/// ceiling they must not. Neither observation passes for the other session.
#[test]
fn a_denied_name_is_found_without_its_ceiling_and_absent_under_it() {
    const NAME: &str = "setup_catalog_entry";
    let row_names = |catalog: &Value| {
        catalog["results"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| row["name"].as_str().unwrap().to_owned())
            .collect::<Vec<_>>()
    };
    let load = |denied: &[&str]| {
        json!({"catalog_exact":true,
            "accepted_names":SETUP_TOOLS.iter().copied().filter(|name| !denied.contains(name)).collect::<Vec<_>>(),
            "noop_names":[],
            "resolutions":SETUP_TOOLS.iter().map(|name| if denied.contains(name) {
                json!({"name":name,"accepted":false,"rejected_reason":"not_filterable"})
            } else {
                json!({"name":name,"accepted":true})
            }).collect::<Vec<_>>()})
    };

    // Without the caller's ceiling (the preparation-only session): found, and
    // its load is accepted.
    let uncapped = ready_catalog(PREPARATION_DENIED);
    assert!(row_names(&uncapped).iter().any(|name| name == NAME));
    assert!(setup_catalog_ready(&uncapped, PREPARATION_DENIED).unwrap());
    setup_load_accepted(&load(PREPARATION_DENIED), PREPARATION_DENIED).unwrap();

    // Under it: absent from the search, and its load keeps the policy refusal.
    let capped = ready_catalog(COMPOSITION_DENIED);
    assert!(!row_names(&capped).iter().any(|name| name == NAME));
    assert!(setup_catalog_ready(&capped, COMPOSITION_DENIED).unwrap());
    setup_load_accepted(&load(COMPOSITION_DENIED), COMPOSITION_DENIED).unwrap();

    // The observations of one session never pass for the other.
    assert!(setup_catalog_ready(&uncapped, COMPOSITION_DENIED).is_err());
    assert!(setup_catalog_ready(&capped, PREPARATION_DENIED).is_err());
    assert!(setup_load_accepted(&load(PREPARATION_DENIED), COMPOSITION_DENIED).is_err());
    assert!(setup_load_accepted(&load(COMPOSITION_DENIED), PREPARATION_DENIED).is_err());
}

#[test]
fn selected_pending_catalog_is_not_ready_even_when_exact() {
    let observed = json!({"catalog_exact":true,"total_matches":0,"results":[],
        "pending_sources":["toolkit-setup"],"empty_result_status":"pending_source"});
    assert!(
        !setup_catalog_ready(&observed, COMPOSITION_DENIED)
            .expect("explicit selected pending source")
    );
    let mut unknown = observed.clone();
    unknown["pending_sources"] = json!(["unselected-server"]);
    assert!(setup_catalog_ready(&unknown, COMPOSITION_DENIED).is_err());
    let mut no_pending = observed;
    no_pending["pending_sources"] = json!([]);
    assert!(
        setup_catalog_ready(&no_pending, COMPOSITION_DENIED).is_err(),
        "stale pending status is not readiness"
    );
}

fn ready_catalog(denied: &[&str]) -> Value {
    let rows = SETUP_TOOLS
        .iter()
        .filter(|name| !denied.contains(name))
        .map(|name| json!({"name":name,"visibility_status":"deferred","currently_callable":false}))
        .collect::<Vec<_>>();
    json!({"catalog_exact":true,"total_matches":rows.len(),"results":rows})
}

#[test]
fn ready_catalog_requires_each_permitted_name_and_no_denied_name() {
    assert!(setup_catalog_ready(&ready_catalog(COMPOSITION_DENIED), COMPOSITION_DENIED).unwrap());
    assert!(setup_catalog_ready(&ready_catalog(PREPARATION_DENIED), PREPARATION_DENIED).unwrap());
    for status in [
        "blocked_by_filter",
        "temporarily_unavailable",
        "pending_source",
        "unknown",
    ] {
        let mut observed = ready_catalog(COMPOSITION_DENIED);
        observed["results"][0]["visibility_status"] = json!(status);
        assert!(
            setup_catalog_ready(&observed, COMPOSITION_DENIED).is_err(),
            "{status} must not admit a permitted load"
        );
    }
    for (status, callable) in [
        ("blocked_by_filter", false),
        ("deferred", false),
        ("loaded", true),
        ("temporarily_unavailable", false),
        ("unknown", false),
    ] {
        let mut observed = ready_catalog(COMPOSITION_DENIED);
        observed["results"].as_array_mut().unwrap().push(json!({
            "name":COMPOSITION_DENIED[0],"visibility_status":status,"currently_callable":callable}));
        assert!(
            setup_catalog_ready(&observed, COMPOSITION_DENIED).is_err(),
            "{status} must not offer a tool the caller's policy denies"
        );
    }
    let mut missing = ready_catalog(COMPOSITION_DENIED);
    missing["results"].as_array_mut().unwrap().pop();
    assert!(
        setup_catalog_ready(&missing, COMPOSITION_DENIED).is_err(),
        "a missing permitted tool is not readiness"
    );
    let mut pending = ready_catalog(COMPOSITION_DENIED);
    pending["pending_sources"] = json!(["toolkit-setup"]);
    assert!(
        !setup_catalog_ready(&pending, COMPOSITION_DENIED).unwrap(),
        "pending owner still prevents load"
    );
}

#[test]
fn load_acceptance_requires_native_resolutions_not_transport_success() {
    // This is the first run's native unknown-key response, generalized over the
    // same exact six names. It is is_error=false at the transport level.
    let mut observed = json!({"catalog_exact":true,"accepted_names":[],"noop_names":[],
        "resolutions":SETUP_TOOLS.iter().map(|name| json!({"name":name,"accepted":false,
            "rejected_reason":"unknown_key"})).collect::<Vec<_>>()});
    assert!(setup_load_accepted(&observed, COMPOSITION_DENIED).is_err());
    observed["accepted_names"] = json!(
        SETUP_TOOLS
            .iter()
            .copied()
            .filter(|name| *name != SETUP_TOOLS[0] && !COMPOSITION_DENIED.contains(name))
            .collect::<Vec<_>>()
    );
    observed["noop_names"] = json!([SETUP_TOOLS[0]]);
    observed["resolutions"] = json!(
        SETUP_TOOLS
            .iter()
            .enumerate()
            .map(|(index, name)| if COMPOSITION_DENIED.contains(name) {
                json!({"name":name,"accepted":false,"rejected_reason":"not_filterable"})
            } else {
                json!({"name":name,"accepted":true,"accepted_noop":index == 0})
            })
            .collect::<Vec<_>>()
    );
    assert!(setup_load_accepted(&observed, COMPOSITION_DENIED).is_ok());
    let accepted = observed.clone();
    for reason in ["unknown_key", "temporarily_unavailable"] {
        let mut wrong_denial = accepted.clone();
        wrong_denial["resolutions"][1]["rejected_reason"] = json!(reason);
        assert!(
            setup_load_accepted(&wrong_denial, COMPOSITION_DENIED).is_err(),
            "{reason} does not prove policy denial"
        );
    }
    let mut accepted_denial = accepted.clone();
    accepted_denial["resolutions"][1]["accepted"] = json!(true);
    assert!(setup_load_accepted(&accepted_denial, COMPOSITION_DENIED).is_err());
    let mut noop_denial = accepted.clone();
    noop_denial["resolutions"][1]["accepted_noop"] = json!(true);
    assert!(setup_load_accepted(&noop_denial, COMPOSITION_DENIED).is_err());
    let mut widened = accepted;
    widened["accepted_names"]
        .as_array_mut()
        .unwrap()
        .push(json!("setup_catalog_entry"));
    assert!(setup_load_accepted(&widened, COMPOSITION_DENIED).is_err());
    observed["resolutions"][2]["rejected_reason"] = json!("temporarily_unavailable");
    assert!(
        setup_load_accepted(&observed, COMPOSITION_DENIED).is_err(),
        "accepted summary cannot hide a rejected resolution"
    );
    observed["resolutions"][2]
        .as_object_mut()
        .unwrap()
        .remove("rejected_reason");
    observed["accepted_names"].as_array_mut().unwrap().pop();
    assert!(
        setup_load_accepted(&observed, COMPOSITION_DENIED).is_err(),
        "partial accepted names cannot enable dispatch"
    );
}

#[tokio::test]
async fn probe_does_not_emit_an_unoffered_tool_call() {
    use futures_util::StreamExt;
    let directory = tempfile::tempdir().unwrap();
    let probe = Probe {
        inner: meerkat_client::TestClient::for_provider(Provider::OpenAI),
        command: Mutex::new(Some(Command {
            id: "invisible".into(),
            name: "setup_catalog".into(),
            args: json!({}),
            issued: false,
            result: None,
        })),
        trace: directory.path().join("refusal.jsonl"),
        stall: std::sync::atomic::AtomicBool::new(false),
        started: tokio::sync::Notify::new(),
    };
    let request = LlmRequest::new("gpt-5.4", vec![]);
    let result = probe.stream(&request).next().await.unwrap();
    assert!(matches!(result, Err(LlmError::InvalidRequest { .. })));
    assert!(!probe.command.lock().unwrap().as_ref().unwrap().issued);
    let trace = std::fs::read_to_string(&probe.trace).unwrap();
    assert!(trace.contains("test_call_refused_not_visible"));
    assert!(!trace.contains("native_model_call"));
}

async fn exercise(session: &ConnectSession, probe: &Probe) -> Result {
    await_setup_catalog(session, probe, COMPOSITION_DENIED).await?;
    let loaded = value(
        call(
            session,
            probe,
            "tool_catalog_load",
            json!({"names":SETUP_TOOLS}),
        )
        .await?,
    )?;
    setup_load_accepted(&loaded, COMPOSITION_DENIED)?;
    // Ask the offered native catalog loader about all observed names. Its
    // exact refusals keep the caller and preparation ceilings in force
    // without making the model emit an unoffered protected tool call; that
    // the ceilings exist is read host side (`require_setup_ceiling`).
    probe.record(
        json!({"kind":"native_catalog_policy_denials_verified","names":COMPOSITION_DENIED}),
    );
    let catalog = value(call(session, probe, "setup_catalog", json!({})).await?)?;
    let entries = catalog["entries"]
        .as_array()
        .ok_or("catalog entries missing")?;
    require(
        entries.len() >= 100,
        "the distributed 100-plus catalog did not reach the native agent",
    )?;
    require(
        catalog["scope"] == "catalog_metadata_only",
        "catalog was misrepresented as installed state",
    )?;
    let before = value(call(session, probe, "setup_state", json!({})).await?)?;
    require(
        before["job_history"]["count"] == 0,
        "a denied action created a job",
    )?;
    let preparation = value(
        call(
            session,
            probe,
            "setup_prepare",
            json!({"family":"services"}),
        )
        .await?,
    )?;
    let job = preparation["job"]["id"]
        .as_str()
        .ok_or("preparation job missing")?
        .to_owned();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(120);
    loop {
        let state = value(call(session, probe, "setup_state", json!({})).await?)?;
        require(
            state["job"]["id"] == job,
            "preparation was unexpectedly replaced",
        )?;
        if state["job"]["status"] != "running" {
            require(
                state["job"]["status"] == "succeeded",
                "real preparation did not succeed; see retained output",
            )?;
            let family = state["families"]
                .as_array()
                .ok_or("families missing")?
                .iter()
                .find(|row| row["id"] == "services")
                .ok_or("Services family missing")?;
            require(family["status"] == "prepared", "family not prepared")?;
            require(
                family["profiles"]
                    .as_array()
                    .ok_or("profiles missing")?
                    .iter()
                    .any(|row| row["id"] == "gmail"),
                "prepared Gmail profile missing",
            )?;
            break;
        }
        require(
            tokio::time::Instant::now() < deadline,
            "preparation did not settle within acceptance bound",
        )?;
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    let help = value(
        call(
            session,
            probe,
            "setup_help",
            json!({"family":"services","profile":"gmail"}),
        )
        .await?,
    )?;
    require(
        help["profile"] == "gmail"
            && !help["operations"]
                .as_array()
                .ok_or("operations missing")?
                .is_empty(),
        "real Gmail operation help missing",
    )?;
    require(
        !help["fields"]
            .as_array()
            .ok_or("fields missing")?
            .is_empty(),
        "real Gmail configuration fields missing",
    )?;
    let failure = call(
        session,
        probe,
        "setup_help",
        json!({"family":"services","profile":"not-a-real-profile"}),
    )
    .await?;
    require(
        failure.is_error && failure.text_content().contains("unknown_profile"),
        "real adapter failure was not retained",
    )?;
    probe.stall.store(true, std::sync::atomic::Ordering::SeqCst);
    let (input, completion) = session
        .submit("Discuss a Telegram frontend for my family without activating a host".into())
        .await?;
    probe.started.notified().await;
    session.cancel(&input).await?;
    let outcome = completion.wait().await?;
    probe.record(json!({"kind":"native_cancellation","outcome":format!("{outcome:?}")}));
    require(
        matches!(outcome, CompletionOutcome::Cancelled),
        "native cancelled input did not settle",
    )?;
    probe
        .stall
        .store(false, std::sync::atomic::Ordering::SeqCst);
    let after = value(call(session, probe, "setup_state", json!({})).await?)?;
    require(
        after["job_history"]["count"] == 1 && after["job"]["id"] == job,
        "denial, failed help or cancellation duplicated a software preparation effect",
    )?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires the digest-bound assembled product; copy this test executable into its bin directory and run outside checkout with an empty PATH"]
async fn installed_product_native_catalog_preparation_and_help() -> Result {
    let current = std::env::current_dir()?;
    let checkout = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(std::path::Path::parent)
        .ok_or("compiled checkout location unavailable")?;
    require(
        !current.starts_with(checkout),
        "acceptance must run outside the checkout",
    )?;
    let path = std::env::var_os("PATH").ok_or("explicit empty-runtime PATH required")?;
    for directory in std::env::split_paths(&path) {
        require(
            directory.is_absolute() && directory.is_dir(),
            "PATH must name an explicit absolute empty directory",
        )?;
        for name in ["python", "python3", "python3.11", "node", "npm"] {
            require(
                !directory.join(name).exists(),
                "PATH contains a forbidden runtime",
            )?;
        }
    }
    let artifacts = tempfile::Builder::new()
        .prefix("connect-product-acceptance-")
        .tempdir()?
        .keep();
    eprintln!(
        "Retained product-composition acceptance: {}",
        artifacts.display()
    );
    let selection = RealmOptions {
        context_root: current,
        state_root: Some(artifacts.join("state")),
        user_config_root: Some(artifacts.join("home")),
    };
    let storage = selection.resolve("product-acceptance")?;
    let realm_directory =
        meerkat_store::realm_paths_in(&storage.locator.state_root, storage.locator.realm.as_str())
            .root;
    let prepared = InstalledResources::locate()?
        .prepare(&storage, std::future::pending())
        .await?;
    require(
        !realm_directory.exists(),
        "preparation created a native realm before admission",
    )?;
    let probe = Arc::new(Probe {
        inner: meerkat_client::TestClient::for_provider(Provider::OpenAI),
        command: Mutex::new(None),
        trace: artifacts.join("native-dispatch.jsonl"),
        stall: std::sync::atomic::AtomicBool::new(false),
        started: tokio::sync::Notify::new(),
    });
    probe.record(
        json!({"kind":"prepared_resources","artifact_directory":prepared.artifact_directory}),
    );
    let options = prepared.connection.preparation_session(SessionOptions {
        model: meerkat::CreateSessionModelResolutionRequest {
            model: Some("gpt-5.4".into()),
            ..Default::default()
        },
        build: SessionBuildOptions {
            llm_client_override: Some(meerkat::encode_llm_client_override_for_service(
                probe.clone(),
            )),
            tool_access_policy: Some(ToolAccessPolicy::DenyList(
                ["setup_catalog_entry"].into_iter().collect(),
            )),
            ..Default::default()
        },
    })?;
    // Host side: the caller and preparation ceilings are in force in the
    // launch policy this session is built with. The ceiling is read here,
    // not from a search row.
    require_setup_ceiling(&options, COMPOSITION_DENIED)?;
    probe.record(json!({"kind":"host_launch_policy_ceiling_verified","names":COMPOSITION_DENIED}));
    let session = ConnectSession::open_with_config(storage, Config::default(), options).await?;
    // On assertion errors close through the same exact native owner before
    // returning. Test deadlines do not drop a live product session or replay it.
    let result = exercise(&session, &probe).await;
    let cleanup = session.close().await;
    probe.record(
        json!({"kind":"native_teardown","result":format!("{cleanup:?}"),
        "exercise":result.as_ref().err().map(ToString::to_string)}),
    );
    cleanup?;
    result
}

#[path = "packaged_setup/active_worker.rs"]
mod active_worker;

#[tokio::test]
#[ignore = "requires the digest-bound assembled product; run its packaged_setup executable outside checkout with an empty PATH"]
async fn installed_product_native_shutdown_joins_active_worker() -> Result {
    active_worker::run().await
}
