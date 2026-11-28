//! A mob profile's `tools.web_search` switch: what a profile that states it
//! (`true` or `false`) gives its members, and that a profile that omits it
//! changes nothing.

use std::collections::BTreeMap;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use meerkat_client::{LlmClient, LlmEvent, LlmRequest, TestClient};
use meerkat_core::ToolCategoryOverride;
use meerkat_core::web_search::{WEB_SEARCH_TOOL_NAME, WebSearchRequest, WebSearchResult};

use crate::build::{BuildAgentConfigParams, build_agent_config};
use crate::definition::{MobDefinition, ParsedMobDefinition};
use crate::ids::{AgentIdentity, ProfileName};
use crate::portable_profile::{project_portable_profile, rehydrate_portable_profile};
use crate::profile::{Profile, ToolConfig};

/// How many definitions this module has made in this process.
static DEFINITIONS: AtomicUsize = AtomicUsize::new(0);

/// A mob id that no other definition of this module has.
///
/// A member's name in the in-process comms registry is its
/// `mob/profile/member` join, the registry is one per process, and it
/// refuses a name while another live member holds it. Every row here builds
/// the `worker` member `worker-1`, and each member a row builds through the
/// factory is built from a definition of its own, so a mob id per definition
/// gives each of those members a name of its own. Rows that run side by side
/// in one process (plain `cargo test`), and two members one row builds, then
/// never claim the same name.
fn own_mob_id() -> String {
    format!(
        "search-desk-{}",
        DEFINITIONS.fetch_add(1, Ordering::Relaxed)
    )
}

/// A one-profile definition, in a mob of its own, whose `worker` tools table
/// is `tools`.
fn parsed(tools: &str) -> ParsedMobDefinition {
    let toml = format!(
        "[mob]\nid = \"{mob}\"\n\n[profiles.worker]\nmodel = \"claude-sonnet-4-5\"\n\n\
         [profiles.worker.tools]\ncomms = true\n{tools}\n",
        mob = own_mob_id()
    );
    MobDefinition::parse_toml(&toml).expect("the definition parses")
}

fn worker(definition: &MobDefinition) -> &Profile {
    definition.profiles[&ProfileName::from("worker")]
        .as_inline()
        .expect("an inline profile")
}

/// The build of a `worker` member of `definition` that runs on `profile`.
async fn build_of(definition: &MobDefinition, profile: &Profile) -> meerkat::AgentBuildConfig {
    build_under(definition, profile, None).await
}

/// The same build under a spawn's own tool access policy.
async fn build_under(
    definition: &MobDefinition,
    profile: &Profile,
    tool_access_policy: Option<meerkat_core::ops::ToolAccessPolicy>,
) -> meerkat::AgentBuildConfig {
    build_agent_config(BuildAgentConfigParams {
        mob_id: &definition.id,
        profile_name: &ProfileName::from("worker"),
        agent_identity: &AgentIdentity::from("worker-1"),
        profile,
        definition,
        realm_profile_store: None,
        external_tools: None,
        compaction_curator_override: None,
        context: None,
        labels: None,
        additional_instructions: None,
        shell_env: None,
        mob_tool_authority_context: None,
        mob_incarnation: Some(crate::tests::fresh_live_incarnation()),
        tool_access_policy,
        inherited_tool_filter: None,
        system_prompt_override: None,
    })
    .await
    .expect("the member's build config")
}

/// The web search switch of the build of a `worker` member on `profile`.
async fn switch_of(definition: &MobDefinition, profile: &Profile) -> ToolCategoryOverride {
    build_of(definition, profile).await.override_web_search
}

/// The two explicit values and the switch each one sets on the build.
const EXPLICIT: [(&str, ToolCategoryOverride); 2] = [
    ("web_search = true", ToolCategoryOverride::Enable),
    ("web_search = false", ToolCategoryOverride::Disable),
];

#[tokio::test]
async fn a_profile_that_enables_web_search_builds_its_member_with_it_enabled() {
    let definition = parsed("web_search = true").definition;
    assert_eq!(
        switch_of(&definition, worker(&definition)).await,
        ToolCategoryOverride::Enable,
        "the profile's `web_search = true` is the build's explicit enable"
    );
}

#[tokio::test]
async fn a_profile_that_disables_web_search_builds_its_member_with_it_disabled() {
    let definition = parsed("web_search = false").definition;
    assert_eq!(
        switch_of(&definition, worker(&definition)).await,
        ToolCategoryOverride::Disable,
        "the profile's `web_search = false` is the build's explicit disable"
    );
}

#[test]
fn web_search_is_a_key_the_tools_table_declares() {
    for (tools, _) in EXPLICIT {
        let parsed = parsed(tools);
        assert_eq!(
            parsed.unknown_profile_keys,
            Vec::new(),
            "`{tools}` is read, not reported as an ignored key"
        );
        assert!(parsed.diagnostics().is_empty(), "no warning for `{tools}`");
    }
}

/// A stored definition (JSON) and a tools table written back to TOML keep
/// the value the profile named.
#[tokio::test]
async fn an_explicit_web_search_value_survives_the_stored_and_toml_round_trips() {
    for (tools, expected) in EXPLICIT {
        let definition = parsed(tools).definition;

        let stored = serde_json::to_string(&definition).expect("the definition serializes");
        let reread: MobDefinition = serde_json::from_str(&stored).expect("and reads back");
        assert_eq!(
            switch_of(&reread, worker(&reread)).await,
            expected,
            "`{tools}` after a stored round trip"
        );

        let table = toml::to_string(&worker(&definition).tools).expect("the tools table");
        let mut rewritten = worker(&definition).clone();
        rewritten.tools = toml::from_str::<ToolConfig>(&table).expect("and reads back");
        assert_eq!(
            switch_of(&definition, &rewritten).await,
            expected,
            "`{tools}` after a TOML round trip of the tools table"
        );
    }
}

/// A read-only profile that enables web search asks its build for both: the
/// explicit enable and the read-only restriction.
#[tokio::test]
async fn a_read_only_profile_that_enables_web_search_builds_with_both() {
    let definition = parsed("read_only = true\nweb_search = true").definition;
    let build = build_of(&definition, worker(&definition)).await;
    assert_eq!(build.override_web_search, ToolCategoryOverride::Enable);
    assert!(
        build
            .declared_tool_restriction
            .as_ref()
            .is_some_and(|restriction| restriction.read_only),
        "the build still declares the profile read-only"
    );
}

/// The portable profile, which a placed member and a stored identity's
/// member are built from, carries the value both ways.
#[tokio::test]
async fn an_explicit_web_search_value_survives_the_portable_profile() {
    for (tools, expected) in EXPLICIT {
        let definition = parsed(tools).definition;
        let profile = worker(&definition);
        let portable = project_portable_profile(
            profile,
            profile.runtime_mode,
            &BTreeMap::new(),
            "worker-1",
            "worker",
            Vec::new(),
        )
        .expect("the profile is portable");
        let rehydrated = rehydrate_portable_profile(&portable).expect("and rehydrates");
        assert_eq!(
            switch_of(&definition, &rehydrated).await,
            expected,
            "`{tools}` after the portable round trip"
        );
    }
}

/// The portable profile of a profile that enables web search.
fn enabling_portable_profile(
    definition: &MobDefinition,
) -> meerkat_contracts::wire::PortableProfile {
    let profile = worker(definition);
    project_portable_profile(
        profile,
        profile.runtime_mode,
        &BTreeMap::new(),
        "worker-1",
        "worker",
        Vec::new(),
    )
    .expect("the profile is portable")
}

/// The portable profile states an enable the profile names.
#[test]
fn the_portable_profile_states_an_enabled_web_search() {
    let definition = parsed("web_search = true").definition;
    let wire = serde_json::to_value(enabling_portable_profile(&definition))
        .expect("the portable profile serializes");
    assert_eq!(
        wire["tools"]["web_search"],
        serde_json::json!(true),
        "the portable tools state the enable: {}",
        wire["tools"]
    );
}

/// A portable profile that states `web_search` on the wire is read, and its
/// member is built with it.
#[tokio::test]
async fn a_portable_profile_that_states_web_search_is_read() {
    let definition = parsed("web_search = true").definition;
    let mut wire = serde_json::to_value(enabling_portable_profile(&definition))
        .expect("the portable profile serializes");
    wire["tools"]["web_search"] = serde_json::json!(true);
    let read: meerkat_contracts::wire::PortableProfile =
        serde_json::from_value(wire).expect("a portable profile stating web_search is read");
    let rehydrated = rehydrate_portable_profile(&read).expect("and rehydrates");
    assert_eq!(
        switch_of(&definition, &rehydrated).await,
        ToolCategoryOverride::Enable
    );
}

/// The public profile projection states an enable the profile names.
#[test]
fn the_wire_profile_states_an_enabled_web_search() {
    let definition = parsed("web_search = true").definition;
    let wire = serde_json::to_value(crate::runtime::profile_to_wire(worker(&definition)))
        .expect("the wire profile serializes");
    assert_eq!(
        wire["tools"]["web_search"],
        serde_json::json!(true),
        "the wire tools state the enable: {}",
        wire["tools"]
    );
}

// ---------------------------------------------------------------------------
// A profile that omits the key: nothing changes for it.
// ---------------------------------------------------------------------------

/// A profile that does not state the key leaves web search to the build, as
/// before the key existed.
#[tokio::test]
async fn a_profile_without_the_key_leaves_web_search_to_the_build() {
    let definition = parsed("").definition;
    assert_eq!(
        switch_of(&definition, worker(&definition)).await,
        ToolCategoryOverride::Inherit,
        "a profile that omits `web_search` names no choice"
    );
}

/// A profile that does not state the key serializes none: the stored
/// definition, the tools table, the portable profile and the wire profile
/// keep the bytes they had.
#[test]
fn a_profile_without_the_key_serializes_none() {
    let definition = parsed("").definition;
    let profile = worker(&definition);
    let portable = project_portable_profile(
        profile,
        profile.runtime_mode,
        &BTreeMap::new(),
        "worker-1",
        "worker",
        Vec::new(),
    )
    .expect("the profile is portable");
    for (form, text) in [
        (
            "the stored definition",
            serde_json::to_string(&definition).expect("the definition serializes"),
        ),
        (
            "the tools table",
            toml::to_string(&profile.tools).expect("the tools table"),
        ),
        (
            "the portable profile",
            serde_json::to_string(&portable).expect("the portable profile serializes"),
        ),
        (
            "the wire profile",
            serde_json::to_string(&crate::runtime::profile_to_wire(profile))
                .expect("the wire profile serializes"),
        ),
    ] {
        assert!(
            !text.contains("web_search"),
            "{form} of a profile that omits the key states none: {text}"
        );
    }
}

// ---------------------------------------------------------------------------
// The member's built agent: what each value gives it.
// ---------------------------------------------------------------------------

/// Records, per model request, whether the provider's own web search rode
/// it and which ordinary tools were offered.
struct SearchCapture {
    inner: TestClient,
    seen: Mutex<Vec<(bool, Vec<String>)>>,
}

impl SearchCapture {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            inner: TestClient::for_provider(meerkat_core::Provider::Anthropic),
            seen: Mutex::default(),
        })
    }

    /// The first request: whether native search rode it, and its tools.
    fn first(&self) -> (bool, Vec<String>) {
        self.seen
            .lock()
            .expect("capture lock")
            .first()
            .cloned()
            .expect("one model request")
    }
}

#[async_trait]
impl LlmClient for SearchCapture {
    fn project_replay_messages(
        &self,
        messages: &[meerkat_core::Message],
    ) -> Result<Vec<meerkat_core::Message>, meerkat_client::LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(
        &'a self,
        request: &'a LlmRequest,
    ) -> Pin<Box<dyn futures::Stream<Item = Result<LlmEvent, meerkat_client::LlmError>> + Send + 'a>>
    {
        let native = matches!(
            &request.provider_params,
            Some(meerkat_core::lifecycle::run_primitive::ProviderTag::Anthropic(anthropic))
                if anthropic.web_search.is_some()
        );
        self.seen.lock().expect("capture lock").push((
            native,
            request
                .tools
                .iter()
                .map(|tool| tool.name.to_string())
                .collect(),
        ));
        self.inner.stream(request)
    }

    fn provider(&self) -> meerkat_core::Provider {
        self.inner.provider()
    }

    async fn health_check(&self) -> Result<(), meerkat_client::LlmError> {
        self.inner.health_check().await
    }
}

/// Counts the searches it serves: the stand-in for the fallback helper's
/// provider call.
struct CountingSearch(AtomicUsize);

#[async_trait]
impl meerkat_llm_core::WebSearchExecutor for CountingSearch {
    async fn execute_web_search(
        &self,
        request: WebSearchRequest,
    ) -> Result<WebSearchResult, meerkat_llm_core::LlmError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(WebSearchResult::unavailable(request.query, "test executor"))
    }
}

/// What one request of a `worker` member whose tools table is `tools` shows:
/// whether the provider's own search rode it, and whether the ordinary
/// `web_search` tool was offered. `config` is the host's configuration.
async fn first_request_of(tools: &str, config: &meerkat_core::Config) -> (bool, bool) {
    let temp = tempfile::tempdir().expect("a factory root");
    let definition = parsed(tools).definition;
    let capture = SearchCapture::new();
    let mut build = build_of(&definition, worker(&definition)).await;
    build.llm_client_override = Some(capture.clone());
    build.web_search_executor_override = Some(Arc::new(CountingSearch(AtomicUsize::new(0))));
    let mut agent = meerkat::AgentFactory::new(temp.path().join("sessions"))
        .builtins(false)
        .build_agent(build, config)
        .await
        .expect("the member builds");
    agent
        .run("inspect tools".to_string().into())
        .await
        .expect("the member runs a turn");
    let (native, offered) = capture.first();
    (
        native,
        offered.iter().any(|name| name == WEB_SEARCH_TOOL_NAME),
    )
}

/// A member of a profile that omits the key searches exactly as before the
/// key existed: the provider's own web search while the provider's switch is
/// on, none while it is off, and never the ordinary `web_search` tool.
#[tokio::test]
async fn a_member_of_a_profile_without_the_key_searches_as_before() {
    assert_eq!(
        first_request_of("", &meerkat_core::Config::default()).await,
        (true, false),
        "the provider's own search, by the provider's default switch"
    );
    let mut switched_off = meerkat_core::Config::default();
    switched_off.provider_tools.anthropic.web_search = false;
    assert_eq!(
        first_request_of("", &switched_off).await,
        (false, false),
        "no search while the provider's switch is off"
    );
}

/// On a model that searches natively, an ungoverned member whose profile
/// enables web search keeps the provider's own search, and one whose profile
/// disables it has none.
#[tokio::test]
async fn an_ungoverned_member_searches_natively_when_enabled_and_not_at_all_when_disabled() {
    assert_eq!(
        first_request_of("web_search = true", &meerkat_core::Config::default()).await,
        (true, false),
        "`web_search = true`: the provider's own search, no second tool"
    );
    assert_eq!(
        first_request_of("web_search = false", &meerkat_core::Config::default()).await,
        (false, false),
        "`web_search = false`: no search of either kind"
    );
}

/// A stated limit, pinned as it is: on a model that searches natively, an
/// ungoverned member's enable gives no search of either kind, and no error,
/// while that provider's web search switch is off.
#[tokio::test]
async fn an_enable_gives_no_search_on_a_natively_searching_model_while_the_provider_switch_is_off()
{
    let mut switched_off = meerkat_core::Config::default();
    switched_off.provider_tools.anthropic.web_search = false;
    assert_eq!(
        first_request_of("web_search = true", &switched_off).await,
        (false, false),
        "the build succeeds and the member has no search"
    );
}

/// The member of `tools` built with a counting stand-in for the fallback
/// helper: its agent and the stand-in.
async fn member_with_search_stand_in(tools: &str) -> (meerkat::DynAgent, Arc<CountingSearch>) {
    governed_member_with_search_stand_in(tools, None, |_| {}).await
}

/// The same member under a spawn's own tool access policy, its build
/// adjusted by `govern` before the factory builds it.
async fn governed_member_with_search_stand_in(
    tools: &str,
    tool_access_policy: Option<meerkat_core::ops::ToolAccessPolicy>,
    govern: impl FnOnce(&mut meerkat::AgentBuildConfig),
) -> (meerkat::DynAgent, Arc<CountingSearch>) {
    let temp = tempfile::tempdir().expect("a factory root");
    let definition = parsed(tools).definition;
    let executor = Arc::new(CountingSearch(AtomicUsize::new(0)));
    let mut build = build_under(&definition, worker(&definition), tool_access_policy).await;
    govern(&mut build);
    build.llm_client_override = Some(Arc::new(TestClient::for_provider(
        meerkat_core::Provider::Anthropic,
    )));
    build.web_search_executor_override = Some(executor.clone());
    let agent = meerkat::AgentFactory::new(temp.path().join("sessions"))
        .builtins(false)
        .build_agent(build, &meerkat_core::Config::default())
        .await
        .expect("the member builds");
    (agent, executor)
}

fn offers_web_search(agent: &meerkat::DynAgent) -> bool {
    agent
        .tool_scope()
        .visible_tools()
        .iter()
        .any(|tool| tool.name == WEB_SEARCH_TOOL_NAME)
}

/// One `web_search` call of `agent`, and whether its result is an error.
async fn search_once(agent: &mut meerkat::DynAgent) -> Result<bool, meerkat_core::AgentError> {
    agent
        .dispatch_external_tool_call(meerkat_core::ToolCall::new(
            "call-search".to_string(),
            WEB_SEARCH_TOOL_NAME.to_string(),
            serde_json::json!({"query": "fixture query"}),
        ))
        .await
        .map(|outcome| outcome.result.is_error)
}

/// A governed member (a deny list that does not name `web_search`, which
/// turns the provider's own tools off) whose profile enables web search
/// builds, is offered the ordinary `web_search` tool, and a call is served
/// by the fallback helper.
#[tokio::test]
async fn a_governed_member_whose_profile_enables_web_search_is_served_by_the_fallback() {
    let (mut agent, executor) =
        member_with_search_stand_in("deny = [\"shell\"]\nweb_search = true").await;
    assert!(
        offers_web_search(&agent),
        "the governed member is offered the ordinary `web_search` tool"
    );
    let failed = search_once(&mut agent).await.expect("the call dispatches");
    assert!(!failed, "the call is served");
    assert_eq!(
        executor.0.load(Ordering::SeqCst),
        1,
        "the fallback helper served the search"
    );
}

/// A read-only profile that enables web search gets the ordinary
/// `web_search` tool too, and the read-only gate admits it: the tool
/// declares itself read-only.
#[tokio::test]
async fn a_read_only_member_whose_profile_enables_web_search_may_search() {
    let (mut agent, executor) =
        member_with_search_stand_in("read_only = true\nweb_search = true").await;
    assert!(
        offers_web_search(&agent),
        "the read-only member is offered the ordinary `web_search` tool"
    );
    let failed = search_once(&mut agent).await.expect("the call dispatches");
    assert!(!failed, "the read-only gate admits the search");
    assert_eq!(executor.0.load(Ordering::SeqCst), 1);
}

/// A deny list that names `web_search` wins over the profile's enable: no
/// call reaches the fallback helper.
#[tokio::test]
async fn a_deny_list_naming_web_search_wins_over_the_profiles_enable() {
    let (mut agent, executor) =
        member_with_search_stand_in("deny = [\"web_search\"]\nweb_search = true").await;
    let refused = search_once(&mut agent).await.unwrap_or(true);
    assert!(refused, "the denied call is refused");
    assert_eq!(
        executor.0.load(Ordering::SeqCst),
        0,
        "no search reaches the helper"
    );
}

/// Without the profile's enable a governed member has no web search at all:
/// the control for the rows above.
#[tokio::test]
async fn a_governed_member_of_a_profile_without_the_key_has_no_web_search() {
    let (agent, _executor) = member_with_search_stand_in("deny = [\"shell\"]").await;
    assert!(
        !offers_web_search(&agent),
        "a governed member is offered no fallback it did not ask for"
    );
}

/// A profile's enable is the build's explicit enable, so it fails closed: on
/// a model without web search of its own, where the fallback helper cannot
/// be provided either (here the provider's web search is switched off), the
/// member's build is refused.
#[tokio::test]
async fn a_profiles_enable_fails_the_build_where_no_web_search_can_be_provided() {
    let temp = tempfile::tempdir().expect("a factory root");
    let definition = MobDefinition::from_toml(&format!(
        "[mob]\nid = \"{mob}\"\n\n[models.plain]\nprovider = \"anthropic\"\n\n\
         [profiles.worker]\nmodel = \"plain\"\n\n\
         [profiles.worker.tools]\ncomms = true\nweb_search = true\n",
        mob = own_mob_id()
    ))
    .expect("the definition parses");
    let mut build = build_of(&definition, worker(&definition)).await;
    build.llm_client_override = Some(Arc::new(TestClient::for_provider(
        meerkat_core::Provider::Anthropic,
    )));
    let mut config = meerkat_core::Config::default();
    config.provider_tools.anthropic.web_search = false;
    match meerkat::AgentFactory::new(temp.path().join("sessions"))
        .builtins(false)
        .build_agent(build, &config)
        .await
    {
        Err(meerkat::BuildAgentError::CapabilityUnavailable { capability, .. }) => {
            assert_eq!(capability, "web_search");
        }
        Err(other) => panic!("expected the web search capability refusal, got: {other}"),
        Ok(_) => panic!("a build that cannot provide the enabled web search must be refused"),
    }
}

/// The tools table of a profile that gates the `web_search` family and
/// states `switch` (a `web_search` line, or nothing).
fn gated_search_tools(switch: &str) -> String {
    format!("{switch}\n\n[profiles.worker.tools.grant_gated]\nfamilies = [\"web_search\"]")
}

/// The member's own tool grant gate still mounts a gated `web_search`
/// family closed, whatever the profile's switch says.
#[tokio::test]
async fn a_grant_gate_covering_web_search_is_mounted_whatever_the_switch() {
    for switch in ["web_search = true", "web_search = false"] {
        let definition = parsed(&gated_search_tools(switch)).definition;
        let build = build_of(&definition, worker(&definition)).await;
        assert_eq!(
            build.override_web_search,
            ToolCategoryOverride::Enable,
            "`{switch}`: the gated family is mounted, closed by the gate"
        );
        assert!(
            build
                .grant_gated_tools
                .as_ref()
                .is_some_and(|gate| gate.families.contains("web_search")),
            "`{switch}`: the build names the gate"
        );
    }
}

/// A profile that gates the `web_search` family and omits the key builds as
/// before the key existed: the build names no web search choice, so the
/// member's session records none, and names the gate, under which the
/// factory mounts the tool closed.
#[tokio::test]
async fn a_profile_without_the_key_that_gates_web_search_builds_as_before() {
    let definition = parsed(&gated_search_tools("")).definition;
    let build = build_of(&definition, worker(&definition)).await;
    assert_eq!(
        build.override_web_search,
        ToolCategoryOverride::Inherit,
        "the build names no web search choice"
    );
    assert!(
        build
            .grant_gated_tools
            .as_ref()
            .is_some_and(|gate| gate.families.contains("web_search")),
        "the build names the gate"
    );
}

fn tool_names(names: &[&str]) -> meerkat_core::ToolNameSet {
    names.iter().copied().collect()
}

/// A spawn's own allow list that names `web_search` admits the enabled
/// tool; one that does not name it refuses every call.
#[tokio::test]
async fn a_spawns_allow_list_decides_the_enabled_web_search() {
    for (named, admitted) in [(WEB_SEARCH_TOOL_NAME, true), ("send_message", false)] {
        let policy = meerkat_core::ops::ToolAccessPolicy::AllowList(tool_names(&[named]));
        let (mut agent, executor) =
            governed_member_with_search_stand_in("web_search = true", Some(policy), |_| {}).await;
        assert_eq!(
            offers_web_search(&agent),
            admitted,
            "an allow list naming `{named}`: whether `web_search` is offered"
        );
        let failed = search_once(&mut agent).await.unwrap_or(true);
        assert_eq!(
            failed, !admitted,
            "an allow list naming `{named}`: the list decides the call"
        );
        assert_eq!(
            executor.0.load(Ordering::SeqCst),
            usize::from(admitted),
            "a search reaches the helper only when the list names the tool"
        );
    }
}

/// A host's dispatch admission that admits or refuses every call, counting
/// the `web_search` calls it is asked about.
struct SearchAdmission {
    admit: bool,
    asked: AtomicUsize,
}

#[async_trait]
impl meerkat_core::ToolDispatchAdmission for SearchAdmission {
    async fn await_dispatch_admission(
        &self,
        call: meerkat_core::ToolCallView<'_>,
        _context: Option<&meerkat_core::ToolDispatchContext>,
        _effect_kind: meerkat_core::LiveBridgeEffectKind,
    ) -> Result<(), meerkat_core::ToolError> {
        if call.name == WEB_SEARCH_TOOL_NAME {
            self.asked.fetch_add(1, Ordering::SeqCst);
        }
        if self.admit {
            Ok(())
        } else {
            Err(meerkat_core::ToolError::access_denied(call.name))
        }
    }
}

/// Under a host's dispatch admission the enabled `web_search` tool is
/// offered, and the admission decides each call.
#[tokio::test]
async fn a_dispatch_admission_decides_each_enabled_web_search_call() {
    for admit in [true, false] {
        let admission = Arc::new(SearchAdmission {
            admit,
            asked: AtomicUsize::new(0),
        });
        let installed = Arc::clone(&admission);
        let (mut agent, executor) =
            governed_member_with_search_stand_in("web_search = true", None, move |build| {
                build.tool_dispatch_admission = Some(installed);
            })
            .await;
        assert!(
            offers_web_search(&agent),
            "the member is offered the ordinary `web_search` tool"
        );
        let failed = search_once(&mut agent).await.unwrap_or(true);
        assert_eq!(
            admission.asked.load(Ordering::SeqCst),
            1,
            "the admission is asked about the call"
        );
        assert_eq!(failed, !admit, "the admission's answer decides the call");
        assert_eq!(
            executor.0.load(Ordering::SeqCst),
            usize::from(admit),
            "a search reaches the helper only when admitted"
        );
    }
}

/// A host's application tool policy that allows or denies every call,
/// counting the `web_search` calls it evaluates.
struct SearchPolicy {
    id: meerkat_core::PolicyProviderId,
    allow: bool,
    evaluated: Arc<AtomicUsize>,
}

struct SearchPolicySnapshot {
    allow: bool,
    evaluated: Arc<AtomicUsize>,
}

impl meerkat_core::ToolConsequencePolicySnapshot for SearchPolicySnapshot {
    fn provenance(&self) -> meerkat_core::PolicyEvaluationProvenance {
        meerkat_core::PolicyEvaluationProvenance {
            revision: meerkat_core::PolicyRevision(1),
            digest: meerkat_core::PolicyDigest::from_canonical_bytes(b"search-policy"),
        }
    }

    fn evaluate(
        &self,
        request: &meerkat_core::ToolConsequenceRequest,
    ) -> meerkat_core::ToolConsequenceVerdict {
        if request.tool_name.as_str() == WEB_SEARCH_TOOL_NAME {
            self.evaluated.fetch_add(1, Ordering::SeqCst);
        }
        if self.allow {
            meerkat_core::ToolConsequenceVerdict::Allow
        } else {
            meerkat_core::ToolConsequenceVerdict::Deny(meerkat_core::ToolConsequenceDenial::new(
                "not_granted",
                "the host's policy does not grant this tool",
            ))
        }
    }
}

impl meerkat_core::ToolConsequenceNarrowingPolicy for SearchPolicy {
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
        Ok(Arc::new(SearchPolicySnapshot {
            allow: self.allow,
            evaluated: Arc::clone(&self.evaluated),
        }))
    }
}

/// Under a host's application tool policy the enabled `web_search` tool is
/// offered, and the policy decides each call: a policy that does not grant
/// it refuses the call.
#[tokio::test]
async fn an_application_tool_policy_decides_each_enabled_web_search_call() {
    for allow in [true, false] {
        let evaluated = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&evaluated);
        let (mut agent, executor) =
            governed_member_with_search_stand_in("web_search = true", None, move |build| {
                let provider_id =
                    meerkat_core::PolicyProviderId::new("host").expect("a provider id");
                build.tool_consequence_policy_registry = Some(Arc::new(
                    meerkat_core::ToolConsequencePolicyRegistry::new(
                        vec![Arc::new(SearchPolicy {
                            id: provider_id.clone(),
                            allow,
                            evaluated: counted,
                        })],
                        meerkat_core::PolicyEvaluationSupervisorConfig::default(),
                        None,
                    )
                    .expect("a policy registry"),
                ));
                build.application_tool_policy =
                    meerkat_core::ApplicationToolPolicyBinding::Provider {
                        provider_id,
                        policy_id: meerkat_core::PolicyId::new("member-tools")
                            .expect("a policy id"),
                    };
            })
            .await;
        assert!(
            offers_web_search(&agent),
            "the member is offered the ordinary `web_search` tool"
        );
        let failed = search_once(&mut agent).await.unwrap_or(true);
        assert_eq!(
            evaluated.load(Ordering::SeqCst),
            1,
            "the policy evaluates the call"
        );
        assert_eq!(failed, !allow, "the policy's verdict decides the call");
        assert_eq!(
            executor.0.load(Ordering::SeqCst),
            usize::from(allow),
            "a search reaches the helper only when granted"
        );
    }
}
