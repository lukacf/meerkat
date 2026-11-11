//! The identity tools at the agent mob tool wrapper: who is offered them,
//! and what a call that reaches the wrapper is answered.
//!
//! The tools are a member's only while the mob's identity family takes
//! changes, its own grant admits spawn tooling and it is private to no
//! one. The wrapper refuses a private member's call itself
//! (`private_requester`, as it refuses its council), and answers a surface
//! whose family takes no changes as it answers any tool it does not have. The seven tools
//! are entries that may wait behind the deferred catalog.

use std::sync::atomic::{AtomicUsize, Ordering};

use meerkat_core::types::{ToolCallView, ToolDef};
use meerkat_core::{AgentToolDispatcher, ToolDispatchOutcome, ToolError};
use meerkat_mob::AgentIdentity;
use serde_json::json;

use super::child_mob_tests::{Host, lifetime, unique};
use super::*;
use crate::identity_creation::{
    AgentEntered, AgentPage, AgentProvenance, AgentReadiness, AgentSummary, ChangeClass,
    ChangeRequester, ChangesHold, ChangesHoldRequest, ConsentSummary, IDENTITY_CREATE,
    IDENTITY_LIST, IDENTITY_TOOLS, IdentityAgentTools, IdentityChangeCall, IdentityFamily,
    IdentityOutcome, IdentityRead, IdentityReadAnswer, IdentityRefusal, IdentityRefusalKind,
    IdentityToolCallSite, IdentityToolCaller, RequesterKind, Sha256Digest, WithdrawRequest,
    Withdrawn, is_identity_tool,
};

/// A family that counts its calls and refuses every one `change_denied`.
#[derive(Default)]
struct CountingFamily {
    calls: AtomicUsize,
}

#[async_trait]
impl IdentityFamily for CountingFamily {
    async fn change(
        &self,
        _mob: &MobHandle,
        _requester: &ChangeRequester,
        _call: IdentityChangeCall,
    ) -> IdentityOutcome {
        self.calls.fetch_add(1, Ordering::SeqCst);
        IdentityOutcome::refused(IdentityRefusalKind::ChangeDenied {
            change: ChangeClass::Create,
        })
    }

    async fn withdraw(
        &self,
        _mob: &MobHandle,
        _requester: &ChangeRequester,
        _request: WithdrawRequest,
    ) -> Result<Withdrawn, IdentityRefusal> {
        Err(IdentityRefusal::new(IdentityRefusalKind::DecisionNotFound))
    }

    async fn changes_hold(
        &self,
        _mob: &MobHandle,
        _requester: &ChangeRequester,
        _request: ChangesHoldRequest,
    ) -> Result<ChangesHold, IdentityRefusal> {
        Ok(ChangesHold::new(false))
    }

    async fn read(
        &self,
        _mob: &MobHandle,
        _requester: &ChangeRequester,
        read: IdentityRead,
    ) -> Result<IdentityReadAnswer, IdentityRefusal> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        match read {
            // One agent that another agent asked for, with its provenance,
            // as the host's own read shows it.
            IdentityRead::List(_) => Ok(IdentityReadAnswer::List(AgentPage {
                agents: vec![AgentSummary {
                    name: "scout".to_owned(),
                    display_name: Some("Scout".to_owned()),
                    profile: "coordinator".to_owned(),
                    spec_digest: Sha256Digest::of(b"scout"),
                    spec_revision: Some(3),
                    archived: true,
                    readiness: AgentReadiness::Unseated,
                    provenance: Some(AgentProvenance {
                        entered: AgentEntered::Created,
                        requester_kind: RequesterKind::Agent,
                        requester: "another-agent".to_owned(),
                        at_ms: 1_700_000_000_000,
                        consent: ConsentSummary::Decided,
                    }),
                    open_operations: Vec::new(),
                }],
                next: None,
            })),
            _ => Err(IdentityRefusal::new(IdentityRefusalKind::ChangeDenied {
                change: ChangeClass::Create,
            })),
        }
    }
}

/// Stands in for meerkat's mob tools: one tool, in an exact catalog.
struct InnerTools;

#[async_trait]
impl AgentToolDispatcher for InnerTools {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        vec![Arc::new(ToolDef::new(
            "mob_list",
            "List mobs.",
            json!({"type": "object"}),
        ))]
        .into()
    }

    fn tool_catalog_capabilities(&self) -> meerkat_core::ToolCatalogCapabilities {
        meerkat_core::ToolCatalogCapabilities {
            exact_catalog: true,
            may_require_catalog_control_plane: false,
        }
    }

    async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
        Err(ToolError::not_found(call.name))
    }
}

/// A root definition whose one profile has mob tools: its members hold the
/// default spawn grant.
fn definition(mob_id: &str) -> MobDefinition {
    MobDefinition::from_toml(&format!(
        "[mob]\nid = \"{mob_id}\"\n\n[profiles.coordinator]\nmodel = \"gpt-5.5\"\n\
         runtime_mode = \"turn_driven\"\n[profiles.coordinator.tools]\ncomms = true\nmob = true\n\n\
         [profiles.plain]\nmodel = \"gpt-5.5\"\nruntime_mode = \"turn_driven\"\n\
         [profiles.plain.tools]\ncomms = true\n"
    ))
    .expect("root definition")
}

/// A runtime whose identity authority is installed, as a gateway installs
/// it, with `family` attached when given.
async fn runtime_with(
    dir: &Path,
    family: Option<Arc<CountingFamily>>,
) -> (MobRuntime, Arc<crate::identity_first::IdentityRuntime>) {
    let mob_id = unique("identity-tools");
    let host = Host::new();
    let runtime = lifetime(dir, &mob_id, &host, |mut spec| {
        spec.definition = definition(&mob_id);
        spec
    })
    .await
    .expect("lifetime");
    let identity = Arc::new(
        crate::identity_first::IdentityRuntime::new(crate::identity_first::IdentityRuntimeConfig {
            continuity_store: Arc::new(
                crate::identity_first::LocalContinuityStore::in_memory().expect("continuity store"),
            ),
            lease_provider: Arc::new(crate::identity_first::LocalLeaseProvider::new()),
            runtime_instance_id: "identity-tools".to_string(),
            has_runtime_store: true,
            durability_policy: crate::identity_first::DurabilityPolicy::SyncWriteThrough,
            bridge: None,
            default_timeout: None,
        })
        .with_runtime_services(crate::identity_first::AgentRuntimeServices::new(
            runtime.handle().observer(),
        )),
    );
    if let Some(family) = family {
        identity.attach_identity_family(family);
    }
    runtime.install_identity_runtime_authority(Arc::clone(&identity));
    (runtime, identity)
}

/// Spawn the member `name` of the profile with mob tools, with `labels`,
/// and answer its session.
async fn member_session(
    runtime: &MobRuntime,
    name: &str,
    labels: &[(&str, &str)],
) -> meerkat_core::types::SessionId {
    member_session_of(runtime, "coordinator", name, labels).await
}

/// Spawn the member `name` of `profile` with `labels`, and answer its
/// session.
async fn member_session_of(
    runtime: &MobRuntime,
    profile: &str,
    name: &str,
    labels: &[(&str, &str)],
) -> meerkat_core::types::SessionId {
    let identity = AgentIdentity::from(name);
    let labels = labels
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect::<BTreeMap<_, _>>();
    Box::pin(runtime.handle().spawn_spec(
        SpawnMemberSpec::new(ProfileName::from(profile), identity.clone()).with_labels(labels),
    ))
    .await
    .expect("spawn the member");
    runtime
        .handle()
        .resolve_bridge_session_id(&identity)
        .await
        .expect("the member's session")
}

/// The wrapper as the runtime builds it for the member of `session`.
fn surface(
    runtime: &MobRuntime,
    session: meerkat_core::types::SessionId,
) -> AutoWireParentMobToolDispatcher {
    AutoWireParentMobToolDispatcher {
        inner: Arc::new(InnerTools),
        implicit_delegate_retirement_overrides: runtime
            .implicit_delegate_retirement_overrides()
            .expect("the runtime installs the agent mob tools"),
        console_spawn_sink: new_console_spawn_sink_slot(),
        identity_runtime: runtime
            .identity_runtime_slot
            .clone()
            .expect("the identity authority slot"),
        protected_mob_id: runtime.handle().mob_id().to_string(),
        spawner_comms_name: None,
        member_auth_binding: None,
        member_auth_binding_recorded: None,
        realm_profiles: None,
        caller: Some(MobToolCaller::new(
            session,
            runtime.private_owners().clone(),
            runtime
                .agent_mob_mcp_state()
                .as_ref()
                .map_or_else(std::sync::Weak::new, Arc::downgrade),
        )),
    }
}

async fn call(
    surface: &AutoWireParentMobToolDispatcher,
    tool: &str,
    arguments: serde_json::Value,
) -> Result<ToolDispatchOutcome, ToolError> {
    let arguments = serde_json::value::RawValue::from_string(arguments.to_string())
        .expect("the arguments are JSON");
    surface
        .dispatch_with_context(
            ToolCallView {
                id: "fc_0",
                name: tool,
                args: &arguments,
            },
            &meerkat_core::ToolDispatchContext::default(),
        )
        .await
}

fn identity_tools_of(surface: &AutoWireParentMobToolDispatcher) -> Vec<String> {
    surface
        .tools()
        .iter()
        .map(|tool| tool.name.to_string())
        .filter(|tool| is_identity_tool(tool))
        .collect()
}

/// A member private to a person asks for no change of long-term agents: the
/// wrapper offers it no identity tool, and a call of one that reaches the
/// wrapper anyway is refused `private_requester` as a policy denial, the
/// way its council is, before the family is called. A member private to no
/// one is offered the seven tools, and its call reaches the family.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_private_members_identity_tool_call_is_refused_private_requester() {
    let dir = tempfile::tempdir().expect("state dir");
    let family = Arc::new(CountingFamily::default());
    let (runtime, _identity) = runtime_with(dir.path(), Some(Arc::clone(&family))).await;
    let private = surface(
        &runtime,
        member_session(
            &runtime,
            "side",
            &[(crate::access::PRIVATE_TO_LABEL, "alice")],
        )
        .await,
    );
    let shared = surface(&runtime, member_session(&runtime, "shared", &[]).await);
    let create = json!({
        "request_id": "r-1",
        "agents": [{"name": "scout", "profile": "coordinator"}],
    });

    assert!(
        identity_tools_of(&private).is_empty(),
        "a private member is offered no identity tool"
    );
    for (tool, arguments) in [
        (IDENTITY_CREATE, create.clone()),
        (IDENTITY_LIST, json!({})),
    ] {
        match call(&private, tool, arguments).await {
            Err(ToolError::PolicyDenied { denial }) => {
                assert_eq!(denial.code, "private_requester", "{}", denial.message);
            }
            other => panic!("a private member's {tool} is refused as a policy denial: {other:?}"),
        }
    }
    assert_eq!(
        family.calls.load(Ordering::SeqCst),
        0,
        "the family was never called for the private member"
    );

    assert_eq!(
        identity_tools_of(&shared),
        IDENTITY_TOOLS,
        "a member private to no one, with a spawn grant, is offered the seven tools"
    );
    // A member whose build bound no mob authority holds no spawn grant: it
    // is offered none, whoever built a surface for its session.
    let plain = surface(
        &runtime,
        member_session_of(&runtime, "plain", "plain-member", &[]).await,
    );
    assert!(
        identity_tools_of(&plain).is_empty(),
        "a member with no spawn grant is offered no identity tool"
    );
    let answer = call(&shared, IDENTITY_CREATE, create)
        .await
        .expect("a shared member's call is answered");
    assert_eq!(
        family.calls.load(Ordering::SeqCst),
        1,
        "the shared member's call reached the family: {answer:?}"
    );

    // A dispatch that names another session as the one the call ran in is
    // not a call of this surface's member: whom it acts for is unknown, so
    // nobody is asked.
    let arguments = serde_json::value::RawValue::from_string(json!({}).to_string())
        .expect("the arguments are JSON");
    let elsewhere = meerkat_core::ToolDispatchContext::default()
        .with_runtime_identity(meerkat_core::types::SessionId::new(), None);
    let outcome = shared
        .dispatch_with_context(
            ToolCallView {
                id: "fc_0",
                name: IDENTITY_LIST,
                args: &arguments,
            },
            &elsewhere,
        )
        .await;
    assert!(
        outcome.is_err(),
        "a call that ran in another session is not answered: {outcome:?}"
    );
    assert_eq!(family.calls.load(Ordering::SeqCst), 1);
    runtime.clear_identity_runtime_authority();
}

/// A mob whose identity family takes no changes has no identity tools: the
/// wrapper offers none, and answers a call of one as a tool that does not
/// exist. That is a mob with no family at all, a mob whose family serves
/// reads only (its launch has no identity policy, or one that is not
/// enabled), and a mob whose identity authority is gone.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_mob_whose_family_takes_no_changes_has_no_identity_tools() {
    let dir = tempfile::tempdir().expect("state dir");
    let (runtime, identity) = runtime_with(dir.path(), None).await;
    let member = surface(&runtime, member_session(&runtime, "shared", &[]).await);
    let absent = |outcome: Result<ToolDispatchOutcome, ToolError>| {
        matches!(outcome, Err(ToolError::NotFound { .. }))
    };

    assert!(identity_tools_of(&member).is_empty());
    assert!(
        absent(call(&member, IDENTITY_LIST, json!({})).await),
        "with no family a call answers as an unknown tool"
    );

    let family = Arc::new(CountingFamily::default());
    identity.attach_identity_reads(Arc::clone(&family) as Arc<dyn IdentityFamily>);
    assert!(
        identity_tools_of(&member).is_empty(),
        "a family that serves reads only mounts no identity tool"
    );
    for tool in [IDENTITY_LIST, IDENTITY_CREATE] {
        assert!(
            absent(call(&member, tool, json!({})).await),
            "with a reads-only family a call of {tool} answers as an unknown tool"
        );
    }
    assert_eq!(
        family.calls.load(Ordering::SeqCst),
        0,
        "the reads-only family was never called by an agent"
    );

    identity.attach_identity_family(Arc::clone(&family) as Arc<dyn IdentityFamily>);
    assert_eq!(
        identity_tools_of(&member),
        IDENTITY_TOOLS,
        "a family that takes changes after the surface was built reaches it"
    );
    assert!(call(&member, IDENTITY_LIST, json!({})).await.is_ok());

    runtime.clear_identity_runtime_authority();
    assert!(
        identity_tools_of(&member).is_empty(),
        "once the identity authority is gone the tools are gone"
    );
    assert!(absent(call(&member, IDENTITY_LIST, json!({})).await));
}

/// An agent that lists the mob's agents is not shown who asked for another
/// agent, when, or under which consent: `identity_list` carries no
/// provenance, though the family's page (the host's read) has it. What the
/// agent needs to act stays: the name, the profile, the spec digest and
/// revision, whether it is archived, its readiness.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn identity_list_shows_an_agent_no_provenance() {
    let dir = tempfile::tempdir().expect("state dir");
    let family = Arc::new(CountingFamily::default());
    let (runtime, _identity) = runtime_with(dir.path(), Some(Arc::clone(&family))).await;
    let session = member_session(&runtime, "shared", &[]).await;
    let answer = IdentityAgentTools::new(family, runtime.handle())
        .call(
            &IdentityToolCaller::new("shared", session),
            &IdentityToolCallSite::outside_a_run("fc_0"),
            IDENTITY_LIST,
            "{}",
        )
        .await
        .expect("the list answers");
    assert_eq!(
        answer.content,
        json!({
            "agents": [{
                "name": "scout",
                "display_name": "Scout",
                "profile": "coordinator",
                "spec_digest": Sha256Digest::of(b"scout").as_str(),
                "spec_revision": 3,
                "archived": true,
                "readiness": "unseated",
            }],
        }),
        "the page as the family gave it, without the agent's provenance"
    );
    let text = answer.text();
    for hidden in ["provenance", "another-agent", "consent", "at_ms", "decided"] {
        assert!(
            !text.contains(hidden),
            "the list names no `{hidden}`: {text}"
        );
    }
    runtime.clear_identity_runtime_authority();
}

/// The seven tools are the wrapper's own exact catalog entries, after the
/// inner ones, and may wait behind the deferred catalog: with an exact
/// inner catalog the session's catalog stays exact, a member's surface
/// says it may need the catalog's control plane, and the seven are what a
/// deferred catalog leaves out of the request until they are loaded.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_identity_tools_are_deferred_entries_of_the_wrappers_catalog() {
    let dir = tempfile::tempdir().expect("state dir");
    let family = Arc::new(CountingFamily::default());
    let (runtime, _identity) = runtime_with(dir.path(), Some(family)).await;
    let member = surface(&runtime, member_session(&runtime, "shared", &[]).await);

    let capabilities = member.tool_catalog_capabilities();
    assert!(capabilities.exact_catalog, "the inner exactness holds");
    assert!(capabilities.may_require_catalog_control_plane);

    let catalog = member.tool_catalog();
    let names: Vec<String> = catalog
        .iter()
        .map(|entry| entry.tool.name.to_string())
        .collect();
    assert_eq!(names[0], "mob_list");
    assert_eq!(names[1..], IDENTITY_TOOLS);
    assert_eq!(
        names,
        member
            .tools()
            .iter()
            .map(|tool| tool.name.to_string())
            .collect::<Vec<_>>(),
        "the catalog lists the tools in names and order"
    );
    assert_eq!(
        catalog[0].deferred_eligibility,
        meerkat_core::ToolCatalogDeferredEligibility::InlineOnly
    );
    for entry in &catalog[1..] {
        assert!(
            matches!(
                entry.deferred_eligibility,
                meerkat_core::ToolCatalogDeferredEligibility::DeferredEligible { .. }
            ),
            "{} may wait behind the deferred catalog",
            entry.tool.name
        );
    }
    assert_eq!(
        meerkat_core::select_catalog_mode_from_snapshot(capabilities.exact_catalog, &catalog, &[]),
        meerkat_core::ToolCatalogMode::Deferred,
        "the session's catalog defers them"
    );
    runtime.clear_identity_runtime_authority();
}
