//! A member that is denied the stock tool `ask_user` is not offered it. A
//! profile's `tools.deny`, or a spawn's deny policy for one member, takes
//! the tool out of what that member's model is offered, as it does for
//! every tool. A call the model makes to it anyway is refused as every
//! denied tool's call is (the tool is not allowed by policy), and the
//! member's run goes on. A member without a deny is offered the tool. Real
//! members, built by the member path through the agent factory.
//!
//! The deny works by name, and the stock name is one a deny list may name
//! on every member build. A member whose host supplies its own tool named
//! `ask_user` among the build's external tools carries no stock tool; a
//! profile that denies the name still builds it, and the deny hides and
//! refuses the host's own tool as it does every tool. A name in
//! `tools.deny` that is in no tool vocabulary of the build and is no stock
//! tool's name (a typo) refuses the member's build.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use meerkat_client::{LlmClient, LlmDoneOutcome, LlmError, LlmEvent, LlmRequest};
use meerkat_core::types::StopReason;
use serde_json::json;

const BACKSTOP: Duration = Duration::from_mins(2);
/// What the row sends a member to make its model call `ask_user`, and the id
/// of that call.
const CALL_ASK_USER: &str = "CALL-ASK-LIMA-4: ask which day to release.";
const ASK_USER_CALL_ID: &str = "call-ask-user";
/// What the row sends a member to make its model call a tool that does not
/// exist, and the id of that call.
const CALL_NO_SUCH_TOOL: &str = "CALL-NONE-LIMA-4: call the missing tool.";
const NO_SUCH_TOOL: &str = "no_such_tool_lima";
const NO_SUCH_TOOL_CALL_ID: &str = "call-no-such-tool";
/// The comms tools every member of these mobs has.
const COMMS_TOOLS: [&str; 5] = [
    "peers",
    "reply_to_peer",
    "send_message",
    "send_request",
    "send_response",
];
/// The stdio MCP server of the rows that configure one, and its one tool.
const LISTING: &str = "listing";
const LOOKUP: &str = "lookup";

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

fn listing_server(dir: &Path) -> PathBuf {
    let path = dir.join("listing.sh");
    std::fs::write(&path, LISTING_SERVER).expect("write the listing server");
    path
}

/// One request the model was sent: the names of the tools it offered, and
/// its messages.
#[derive(Clone)]
struct Captured {
    tools: Vec<String>,
    messages: Vec<meerkat_core::Message>,
}

/// A model that calls `ask_user` when its last message is
/// [`CALL_ASK_USER`], calls [`NO_SUCH_TOOL`] when it is
/// [`CALL_NO_SUCH_TOOL`], and answers `ok` to anything else. It keeps every
/// request.
#[derive(Clone, Default)]
struct CallingModel {
    requests: Arc<Mutex<Vec<Captured>>>,
}

impl CallingModel {
    /// The first request, once there is one.
    async fn first_request(&self) -> Captured {
        self.request_where(|_| true, "any request").await
    }

    /// The first request that `matches`, once there is one.
    async fn request_where(&self, matches: impl Fn(&Captured) -> bool, what: &str) -> Captured {
        let deadline = tokio::time::Instant::now() + BACKSTOP;
        loop {
            let found = self
                .requests
                .lock()
                .unwrap()
                .iter()
                .find(|request| matches(request))
                .cloned();
            if let Some(found) = found {
                return found;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "no model request with {what}"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    /// The result the member's model was given for the call `call_id`:
    /// whether it is an error, and its text.
    async fn result_of(&self, call_id: &str) -> (bool, String) {
        let result = |request: &Captured| {
            request.messages.iter().find_map(|message| match message {
                meerkat_core::Message::ToolResults { results, .. } => results
                    .iter()
                    .find(|result| result.tool_use_id == call_id)
                    .map(|result| (result.is_error, result.text_content())),
                _ => None,
            })
        };
        let request = self
            .request_where(|request| result(request).is_some(), call_id)
            .await;
        result(&request).expect("the request holds the result")
    }
}

impl LlmClient for CallingModel {
    fn project_replay_messages(
        &self,
        messages: &[meerkat_core::Message],
    ) -> Result<Vec<meerkat_core::Message>, LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(
        &'a self,
        request: &'a LlmRequest,
    ) -> Pin<Box<dyn futures::Stream<Item = Result<LlmEvent, LlmError>> + Send + 'a>> {
        let last = request
            .messages
            .last()
            .and_then(|last| serde_json::to_string(last).ok())
            .unwrap_or_default();
        self.requests.lock().unwrap().push(Captured {
            tools: request
                .tools
                .iter()
                .map(|tool| tool.name.to_string())
                .collect(),
            messages: request.messages.clone(),
        });
        let call = if last.contains(CALL_ASK_USER) {
            Some((
                ASK_USER_CALL_ID,
                "ask_user",
                json!({
                    "questions": [{
                        "id": "day",
                        "prompt": "Which day?",
                        "choices": [
                            {"id": "mon", "label": "Monday"},
                            {"id": "fri", "label": "Friday"},
                        ],
                    }],
                }),
            ))
        } else if last.contains(CALL_NO_SUCH_TOOL) {
            Some((NO_SUCH_TOOL_CALL_ID, NO_SUCH_TOOL, json!({})))
        } else {
            None
        };
        match call {
            Some((id, name, args)) => Box::pin(async_stream::stream! {
                yield Ok(LlmEvent::ToolCallComplete {
                    id: id.to_string(),
                    name: name.to_string(),
                    args,
                    meta: None,
                });
                yield Ok(LlmEvent::Done {
                    outcome: LlmDoneOutcome::success(StopReason::ToolUse),
                });
            }),
            None => Box::pin(async_stream::stream! {
                yield Ok(LlmEvent::TextDelta {
                    delta: "ok".to_string(),
                    meta: None,
                });
                yield Ok(LlmEvent::Done {
                    outcome: LlmDoneOutcome::success(StopReason::EndTurn),
                });
            }),
        }
    }

    fn provider(&self) -> meerkat::Provider {
        meerkat::Provider::OpenAI
    }

    fn health_check<'life0, 'async_trait>(
        &'life0 self,
    ) -> Pin<Box<dyn Future<Output = Result<(), LlmError>> + Send + 'async_trait>>
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async { Ok(()) })
    }
}

/// What the host's own tool named `ask_user` answers a call.
const HOSTS_OWN_ANSWER: &str = "answered by the host's own tool";

/// A host's own tool named `ask_user`, as a host puts one among a member
/// build's external tools. It records every call that reaches it.
struct HostsOwnAskUser(Arc<Mutex<Vec<String>>>);

#[async_trait::async_trait]
impl meerkat_core::AgentToolDispatcher for HostsOwnAskUser {
    fn tools(&self) -> Arc<[Arc<meerkat_core::types::ToolDef>]> {
        vec![Arc::new(meerkat_core::types::ToolDef::new(
            "ask_user",
            "The host's own tool of this name.",
            json!({"type": "object", "properties": {}}),
        ))]
        .into()
    }

    async fn dispatch(
        &self,
        call: meerkat_core::types::ToolCallView<'_>,
    ) -> Result<meerkat_core::ToolDispatchOutcome, meerkat_core::ToolError> {
        self.0.lock().unwrap().push(call.name.to_string());
        Ok(meerkat_core::ToolDispatchOutcome::sync_result(
            meerkat_core::types::ToolResult::new(
                call.id.to_string(),
                HOSTS_OWN_ANSWER.to_string(),
                false,
            ),
        ))
    }

    fn capabilities(&self) -> meerkat_core::agent::DispatcherCapabilities {
        meerkat_core::agent::DispatcherCapabilities::default()
    }
}

/// How a row's one member is set up.
#[derive(Clone, Copy)]
struct Setup {
    /// The mob's id.
    mob: &'static str,
    /// The profile's `tools.deny` names `ask_user`.
    profile_denies: bool,
    /// The profile's `tools.deny` names `ask_usr`, a name no tool of any
    /// build has.
    profile_mistypes: bool,
    /// The host supplies its own tool named `ask_user` among the member
    /// build's external tools.
    hosts_own_tool: bool,
    /// The profile configures the listing MCP server, which maps no tool
    /// names.
    mcp_server: bool,
    /// The member's spawn carries a deny policy that names `ask_user`.
    spawn_denies: bool,
}

/// What a row observed of its member.
struct Observed {
    /// The tools its first model request offered, sorted.
    offered: Vec<String>,
    /// The result of a call its model made to `ask_user`.
    ask_user: (bool, String),
    /// The result of a call its model made to a tool that does not exist.
    no_such_tool: (bool, String),
    /// The calls that reached the host's own tool named `ask_user`.
    own_tool_calls: Vec<String>,
}

/// [`try_observe`] of a member that builds.
async fn observe(setup: Setup) -> Observed {
    try_observe(setup)
        .await
        .unwrap_or_else(|refused| panic!("the member builds, but its build was refused: {refused}"))
}

/// Build the mob of `setup`, spawn its one member, and observe what its
/// model is offered and what its two calls are answered. A refused build
/// answers its error, after checking that it ran nothing.
async fn try_observe(setup: Setup) -> Result<Observed, String> {
    let dir = tempfile::tempdir().unwrap();
    let mut config = format!(
        r#"
[mob]
id = "{mob}"

[profiles.worker]
model = "gpt-5.5"
runtime_mode = "autonomous_host"
external_addressable = true

[profiles.worker.tools]
comms = true
"#,
        mob = setup.mob,
    );
    if setup.profile_denies {
        config.push_str("deny = [\"ask_user\"]\n");
    }
    if setup.profile_mistypes {
        config.push_str("deny = [\"ask_usr\"]\n");
    }
    if setup.mcp_server {
        let server = listing_server(dir.path());
        config.push_str(&format!(
            r#"
[[profiles.worker.tools.mcp_servers]]
name = "{LISTING}"
command = "/bin/sh"
args = [{server:?}]
"#,
            server = server.display().to_string(),
        ));
    }
    let definition = meerkat_mob::MobDefinition::from_toml(&config).expect("mob definition");
    let model = CallingModel::default();
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
        default_llm_client: Some(Arc::new(model.clone())),
    });
    let runtime = meerkat_meta::MobRuntime::bootstrap(spec)
        .await
        .expect("bootstrap");
    let mut spawn = meerkat_mob::SpawnMemberSpec::host_root(
        "worker",
        meerkat_mob::AgentIdentity::from("worker-1"),
    );
    if setup.spawn_denies {
        spawn = spawn.with_tool_access_policy(meerkat_core::ops::ToolAccessPolicy::DenyList(
            ["ask_user"].into_iter().collect(),
        ));
    }
    let own_tool_calls = Arc::new(Mutex::new(Vec::new()));
    if setup.hosts_own_tool {
        spawn.external_tools = Some(Arc::new(HostsOwnAskUser(Arc::clone(&own_tool_calls))));
    }
    if let Err(refused) = Box::pin(runtime.handle().spawn_spec(spawn)).await {
        assert!(
            model.requests.lock().unwrap().is_empty(),
            "a refused build runs no turn"
        );
        let _ = runtime.handle().shutdown().await;
        return Err(refused.to_string());
    }
    let send = |text: &'static str| {
        let handle = runtime.handle();
        async move {
            meerkat_meta::send_message_on_mob(&handle, "worker-1", text.to_string())
                .await
                .expect("send");
        }
    };
    send("hello").await;
    let mut offered = model.first_request().await.tools;
    offered.sort();
    send(CALL_ASK_USER).await;
    let ask_user = model.result_of(ASK_USER_CALL_ID).await;
    send(CALL_NO_SUCH_TOOL).await;
    let no_such_tool = model.result_of(NO_SUCH_TOOL_CALL_ID).await;
    let _ = runtime.handle().shutdown().await;
    let own_tool_calls = own_tool_calls.lock().unwrap().clone();
    Ok(Observed {
        offered,
        ask_user,
        no_such_tool,
        own_tool_calls,
    })
}

fn names(tools: &[&str]) -> Vec<String> {
    let mut names: Vec<String> = tools.iter().map(ToString::to_string).collect();
    names.sort();
    names
}

/// What a refused call of a denied tool is told, and what a call of a tool
/// that does not exist is told.
const DENIED: &str = "is not allowed by policy";
const NOT_FOUND: &str = "Tool not found";

/// The member is not offered `ask_user`. Its call to it is refused as a
/// denied tool's call is, an error that says the tool is not allowed, and
/// its run goes on: the call it makes afterwards, to a tool that does not
/// exist, is answered in its turn, as not found.
fn assert_denied(observed: &Observed, offered: &[&str], case: &str) {
    assert_eq!(
        observed.offered,
        names(offered),
        "{case}: the member is offered its own tools and not ask_user"
    );
    let (is_error, text) = &observed.ask_user;
    assert!(
        *is_error && text.contains(DENIED) && text.contains("ask_user"),
        "{case}: a call to the denied ask_user is refused as not allowed by policy: {:?}",
        observed.ask_user
    );
    let (is_error, text) = &observed.no_such_tool;
    assert!(
        *is_error && text.contains(NOT_FOUND),
        "{case}: the member's run went on, and its call to a tool that does not exist is \
         answered as not found: {:?}",
        observed.no_such_tool
    );
}

/// A member whose profile denies `ask_user` builds, is not offered the
/// tool, and a call to it is refused as not allowed.
#[tokio::test(flavor = "multi_thread")]
async fn a_member_whose_profile_denies_ask_user_is_not_offered_it() {
    let observed = observe(Setup {
        mob: "ask-user-denied-profile",
        profile_denies: true,
        profile_mistypes: false,
        hosts_own_tool: false,
        mcp_server: false,
        spawn_denies: false,
    })
    .await;
    assert_denied(&observed, &COMMS_TOOLS, "profile deny");
}

/// The same for a member whose profile also configures an MCP server that
/// maps no tool names: its server's tool and the tool catalog's control
/// tools are offered, `ask_user` is not.
#[tokio::test(flavor = "multi_thread")]
async fn a_member_with_an_mcp_server_whose_profile_denies_ask_user_is_not_offered_it() {
    let observed = observe(Setup {
        mob: "ask-user-denied-profile-mcp",
        profile_denies: true,
        profile_mistypes: false,
        hosts_own_tool: false,
        mcp_server: true,
        spawn_denies: false,
    })
    .await;
    let mut offered = COMMS_TOOLS.to_vec();
    offered.extend([LOOKUP, "tool_catalog_load", "tool_catalog_search"]);
    assert_denied(&observed, &offered, "profile deny with an MCP server");
}

/// A spawn's deny policy for one member does the same for that member.
#[tokio::test(flavor = "multi_thread")]
async fn a_member_whose_spawn_denies_ask_user_is_not_offered_it() {
    let observed = observe(Setup {
        mob: "ask-user-denied-spawn",
        profile_denies: false,
        profile_mistypes: false,
        hosts_own_tool: false,
        mcp_server: false,
        spawn_denies: true,
    })
    .await;
    assert_denied(&observed, &COMMS_TOOLS, "spawn deny");
}

/// A member without a deny is offered `ask_user`, and the tool takes its
/// call: the answer is the tool's own, not a refusal (no person can be
/// asked on this runtime, which serves no Console).
#[tokio::test(flavor = "multi_thread")]
async fn a_member_without_a_deny_is_offered_ask_user() {
    let observed = observe(Setup {
        mob: "ask-user-not-denied",
        profile_denies: false,
        profile_mistypes: false,
        hosts_own_tool: false,
        mcp_server: false,
        spawn_denies: false,
    })
    .await;
    let mut offered = COMMS_TOOLS.to_vec();
    offered.push("ask_user");
    assert_eq!(observed.offered, names(&offered));
    let (is_error, text) = &observed.ask_user;
    assert!(
        !*is_error && text.contains("no_human_surface"),
        "the mounted tool answers its call itself: {:?}",
        observed.ask_user
    );
    let (is_error, text) = &observed.no_such_tool;
    assert!(
        *is_error && text.contains(NOT_FOUND),
        "a call to a tool that does not exist is answered as not found: {:?}",
        observed.no_such_tool
    );
}

/// A host supplies its own tool named `ask_user` among the member build's
/// external tools, so Meta mounts no stock tool. A profile that denies the
/// name is still a profile the member builds on, and the deny does to the
/// host's own tool what it does to every tool: the member's model is not
/// offered it, a call its model makes to it anyway is refused as not allowed
/// by policy and never reaches the host's tool, and the member's run goes
/// on.
#[tokio::test(flavor = "multi_thread")]
async fn a_hosts_own_ask_user_that_the_profile_denies_is_hidden_and_refused() {
    let observed = observe(Setup {
        mob: "ask-user-own-tool-denied",
        profile_denies: true,
        profile_mistypes: false,
        hosts_own_tool: true,
        mcp_server: false,
        spawn_denies: false,
    })
    .await;
    assert_denied(&observed, &COMMS_TOOLS, "the host's own ask_user, denied");
    assert!(
        observed.own_tool_calls.is_empty(),
        "the refused call never reaches the host's own tool: {:?}",
        observed.own_tool_calls
    );
}

/// The same member without the deny is offered the host's own tool under
/// the name `ask_user`, and its call reaches that tool, which answers it.
#[tokio::test(flavor = "multi_thread")]
async fn a_hosts_own_ask_user_that_no_profile_denies_is_offered_and_takes_its_call() {
    let observed = observe(Setup {
        mob: "ask-user-own-tool",
        profile_denies: false,
        profile_mistypes: false,
        hosts_own_tool: true,
        mcp_server: false,
        spawn_denies: false,
    })
    .await;
    let mut offered = COMMS_TOOLS.to_vec();
    offered.push("ask_user");
    assert_eq!(observed.offered, names(&offered));
    assert_eq!(
        observed.ask_user,
        (false, HOSTS_OWN_ANSWER.to_string()),
        "the host's own tool answers the call"
    );
    assert_eq!(observed.own_tool_calls, vec!["ask_user".to_string()]);
    let (is_error, text) = &observed.no_such_tool;
    assert!(
        *is_error && text.contains(NOT_FOUND),
        "a call to a tool that does not exist is answered as not found: {:?}",
        observed.no_such_tool
    );
}

/// A mistyped name in `tools.deny` (`ask_usr`: in no tool vocabulary of the
/// build, and no stock tool's name) still refuses the member's build, typed,
/// naming the profile and the name, with the stock tool mounted and with the
/// host's own tool named `ask_user`. Nothing runs.
#[tokio::test(flavor = "multi_thread")]
async fn a_mistyped_deny_name_still_refuses_the_members_build() {
    for (mob, hosts_own_tool) in [
        ("ask-user-mistyped", false),
        ("ask-user-mistyped-own-tool", true),
    ] {
        let refused = try_observe(Setup {
            mob,
            profile_denies: false,
            profile_mistypes: true,
            hosts_own_tool,
            mcp_server: false,
            spawn_denies: false,
        })
        .await
        .err()
        .unwrap_or_else(|| panic!("{mob}: a mistyped deny name refuses the build"));
        println!("MISTYPED DENY NAME ({mob}): {refused}");
        assert!(
            refused.contains(
                "profile 'worker' denies tool 'ask_usr', which is in none of its tool \
                 vocabularies"
            ),
            "{mob}: {refused}"
        );
    }
}
