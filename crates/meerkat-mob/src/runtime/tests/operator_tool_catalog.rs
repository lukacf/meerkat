//! The mob operator tools report an exact catalog, so a member that mounts
//! them can still use the deferred tool catalog for its other tools.
//!
//! The operator tools themselves stay inline: every catalog entry is the
//! tool the dispatcher lists, in the same order, and none of them may wait
//! behind the deferred catalog. A wrapper around a source that does not
//! report an exact catalog keeps reporting none.

use super::*;
use meerkat_core::types::{ToolProvenance, ToolSourceKind};
use meerkat_core::{
    ToolCatalogCapabilities, ToolCatalogDeferredEligibility, ToolCatalogEntry, ToolPlaneClass,
};

/// The catalog is exact, never asks for the catalog control tools by itself,
/// and is the listed tools in names and order, each an inline session entry.
fn assert_exact_inline_catalog(tools: &dyn AgentToolDispatcher, what: &str) {
    let capabilities = tools.tool_catalog_capabilities();
    assert!(
        capabilities.exact_catalog,
        "{what}: the catalog must be exact"
    );
    assert!(
        !capabilities.may_require_catalog_control_plane,
        "{what}: inline tools never ask for the catalog control tools"
    );
    let listed = tools.tools();
    let catalog = tools.tool_catalog();
    assert_eq!(
        catalog
            .iter()
            .map(|entry| entry.tool.name.to_string())
            .collect::<Vec<_>>(),
        listed
            .iter()
            .map(|tool| tool.name.to_string())
            .collect::<Vec<_>>(),
        "{what}: the catalog lists the tools in names and order"
    );
    for (tool, entry) in listed.iter().zip(catalog.iter()) {
        assert_eq!(
            serde_json::to_value(tool.as_ref()).expect("tool json"),
            serde_json::to_value(entry.tool.as_ref()).expect("entry json"),
            "{what}: the entry carries the listed definition"
        );
        assert_eq!(
            entry.plane,
            ToolPlaneClass::Session,
            "{what}: {}",
            tool.name
        );
        assert_eq!(
            entry.deferred_eligibility,
            ToolCatalogDeferredEligibility::InlineOnly,
            "{what}: {} stays inline",
            tool.name
        );
        assert!(
            entry.callability.is_callable(),
            "{what}: {} is callable",
            tool.name
        );
    }
}

fn operator_authority(handle: &MobHandle) -> meerkat_core::service::MobToolAuthorityContext {
    generated_mob_operator_authority_with_spawn_profile(
        handle.definition().id.as_str(),
        handle.live_incarnation(),
        "worker",
    )
}

/// The dispatcher the controlling side builds with a fixed context.
#[tokio::test]
async fn the_mob_operator_tool_catalog_is_exact_and_mirrors_its_tools() {
    let (handle, _service) = create_test_mob(sample_definition_with_mob_tools()).await;
    let dispatcher = super::super::tools::MobOperatorToolDispatcher::new(
        handle.clone(),
        true,
        operator_authority(&handle),
    );
    assert!(
        dispatcher
            .tools()
            .iter()
            .any(|tool| tool.name == "spawn_member"),
        "the operator tools are listed"
    );
    assert_exact_inline_catalog(&dispatcher, "fixed-context operator tools");

    let disabled = super::super::tools::MobOperatorToolDispatcher::new(
        handle.clone(),
        false,
        operator_authority(&handle),
    );
    assert!(disabled.tools().is_empty());
    assert_exact_inline_catalog(&disabled, "disabled operator tools");
}

/// A local member's own tools follow its session authority: listed while
/// unbound or bound to this mob, hidden while the authority names another
/// incarnation of the mob and once the session holds none. The catalog is
/// the listed tools at every step.
#[tokio::test]
async fn a_members_operator_tool_catalog_follows_what_it_lists() {
    let (handle, _service) = create_test_mob(sample_definition_with_mob_tools()).await;
    let dispatcher = super::super::tools::MobOperatorToolDispatcher::for_member_session(
        handle.clone(),
        true,
        operator_authority(&handle),
    );
    assert!(!dispatcher.tools().is_empty(), "listed before a binding");
    assert_exact_inline_catalog(&dispatcher, "unbound member operator tools");

    dispatcher.bind_session_mob_tool_authority(Some(Arc::new(std::sync::RwLock::new(
        operator_authority(&handle),
    ))));
    assert!(!dispatcher.tools().is_empty(), "listed while bound");
    assert_exact_inline_catalog(&dispatcher, "bound member operator tools");

    // Bound to an authority that names another incarnation of this mob: the
    // tools are hidden as a whole and have no catalog entry, and the source
    // still answers exact. Bound to this incarnation again they are back.
    // The other incarnation is a second mob's: an authority holds this mob's
    // id with that mob's incarnation.
    let mut other_definition = sample_definition_with_mob_tools();
    other_definition.id = MobId::from(format!("{}-other", handle.definition().id.as_str()));
    let (other, _other_service) = create_test_mob(other_definition).await;
    assert_ne!(
        other.incarnation(),
        handle.incarnation(),
        "precondition: the second mob is another incarnation"
    );
    dispatcher.bind_session_mob_tool_authority(Some(Arc::new(std::sync::RwLock::new(
        generated_mob_operator_authority_with_spawn_profile(
            handle.definition().id.as_str(),
            other.live_incarnation(),
            "worker",
        ),
    ))));
    assert!(
        dispatcher.tools().is_empty(),
        "hidden while bound to another incarnation"
    );
    assert!(
        dispatcher.tool_catalog().is_empty(),
        "a tool hidden by another incarnation's authority has no catalog entry"
    );
    assert_exact_inline_catalog(
        &dispatcher,
        "member operator tools bound to another incarnation",
    );
    dispatcher.bind_session_mob_tool_authority(Some(Arc::new(std::sync::RwLock::new(
        operator_authority(&handle),
    ))));
    assert!(
        !dispatcher.tools().is_empty(),
        "listed again once bound to this incarnation"
    );
    assert_exact_inline_catalog(&dispatcher, "rebound member operator tools");

    dispatcher.bind_session_mob_tool_authority(None);
    assert!(dispatcher.tools().is_empty(), "hidden without authority");
    assert!(
        dispatcher.tool_catalog().is_empty(),
        "a hidden tool has no catalog entry"
    );
    assert_exact_inline_catalog(&dispatcher, "hidden member operator tools");
}

/// The tools the mob runtime composes for a spawned member, as the session
/// service receives them.
#[tokio::test]
async fn the_tools_the_runtime_composes_for_a_member_report_an_exact_catalog() {
    let (handle, service) = create_test_mob(sample_definition_with_mob_tools()).await;
    handle
        .spawn(
            ProfileName::from("worker"),
            AgentIdentity::from("w-1"),
            None,
        )
        .await
        .expect("spawn the worker");
    let session_id = handle
        .get_member(&AgentIdentity::from("w-1"))
        .await
        .expect("roster read")
        .expect("roster entry")
        .bridge_session_id()
        .cloned()
        .expect("session-backed member");
    let tools = service
        .external_tools_by_session
        .read()
        .await
        .get(&session_id)
        .cloned()
        .expect("the member's session records its external tools");
    assert!(
        tools.tools().iter().any(|tool| tool.name == "spawn_member"),
        "the member mounts the operator tools"
    );
    assert_exact_inline_catalog(tools.as_ref(), "runtime-composed member tools");
}

/// A host tool source with an exact catalog whose tools may wait behind the
/// deferred catalog.
struct ExactHostTools {
    catalog: Arc<[ToolCatalogEntry]>,
}

impl ExactHostTools {
    fn new(names: &[&str]) -> Self {
        let provenance = ToolProvenance {
            kind: ToolSourceKind::Callback,
            source_id: "host-tools".into(),
            artifact: None,
        };
        let catalog = names
            .iter()
            .map(|name| {
                let mut tool = ToolDef::new(
                    *name,
                    "A host tool",
                    serde_json::json!({ "type": "object" }),
                );
                tool.provenance = Some(provenance.clone());
                ToolCatalogEntry::session_deferred(Arc::new(tool), true, provenance.clone())
            })
            .collect::<Vec<_>>()
            .into();
        Self { catalog }
    }
}

#[async_trait]
impl AgentToolDispatcher for ExactHostTools {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        self.catalog
            .iter()
            .map(|entry| Arc::clone(&entry.tool))
            .collect::<Vec<_>>()
            .into()
    }

    fn tool_catalog_capabilities(&self) -> ToolCatalogCapabilities {
        ToolCatalogCapabilities {
            exact_catalog: true,
            may_require_catalog_control_plane: false,
        }
    }

    fn tool_catalog(&self) -> Arc<[ToolCatalogEntry]> {
        Arc::clone(&self.catalog)
    }

    async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
        Err(ToolError::not_found(call.name))
    }
}

/// A host tool source that reports no exact catalog (the trait default).
struct InexactHostTools {
    tools: Arc<[Arc<ToolDef>]>,
}

impl InexactHostTools {
    fn new(names: &[&str]) -> Self {
        Self {
            tools: names
                .iter()
                .map(|name| {
                    Arc::new(ToolDef::new(
                        *name,
                        "A host tool",
                        serde_json::json!({ "type": "object" }),
                    ))
                })
                .collect::<Vec<_>>()
                .into(),
        }
    }
}

#[async_trait]
impl AgentToolDispatcher for InexactHostTools {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        Arc::clone(&self.tools)
    }

    async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
        Err(ToolError::not_found(call.name))
    }
}

fn worker_profile(handle: &MobHandle) -> crate::profile::Profile {
    handle
        .definition()
        .profiles
        .get(&ProfileName::from("worker"))
        .and_then(|profile| profile.as_inline())
        .expect("inline worker profile")
        .clone()
}

/// Operator tools beside a host's exact tool source compose to one exact
/// catalog: the operator tools inline, the host's tools still eligible to
/// wait behind the deferred catalog.
#[tokio::test]
async fn operator_tools_beside_an_exact_host_source_compose_to_an_exact_catalog() {
    let (handle, _service) = create_test_mob(sample_definition_with_mob_tools()).await;
    let host: Arc<dyn AgentToolDispatcher> =
        Arc::new(ExactHostTools::new(&["host_alpha", "host_beta"]));
    let tools = super::super::tools::compose_external_tools_for_profile(
        &worker_profile(&handle),
        &BTreeMap::new(),
        handle.clone(),
        Some(host),
        None,
        None,
    )
    .expect("compose the member tools")
    .expect("a tools.mob member gets its operator tools");

    assert!(
        tools.tool_catalog_capabilities().exact_catalog,
        "the member's composed tools report an exact catalog"
    );
    let catalog = tools.tool_catalog();
    for entry in catalog.iter() {
        let host_tool = entry.tool.name.starts_with("host_");
        assert_eq!(
            matches!(
                entry.deferred_eligibility,
                ToolCatalogDeferredEligibility::DeferredEligible { .. }
            ),
            host_tool,
            "{}: only the host's tools may wait behind the catalog",
            entry.tool.name
        );
    }
    assert!(
        catalog
            .iter()
            .any(|entry| entry.tool.name == "spawn_member")
    );
    assert_eq!(
        catalog
            .iter()
            .filter(|entry| entry.tool.name.starts_with("host_"))
            .count(),
        2
    );
}

/// A source that reports no exact catalog keeps the member's composed tools
/// inexact, through every wrapper the composition can put around it: no
/// wrapper claims an exactness its source does not report. Each case puts
/// exactly one wrapper around the source, and the row names every wrapper
/// that claimed exactness.
#[tokio::test]
async fn an_inexact_host_source_keeps_the_composed_catalog_inexact() {
    let (handle, _service) = create_test_mob(sample_definition_with_mob_tools()).await;
    let inexact =
        |names: &[&str]| -> Arc<dyn AgentToolDispatcher> { Arc::new(InexactHostTools::new(names)) };
    let compose = |profile: &crate::profile::Profile,
                   bundles: &BTreeMap<String, Arc<dyn AgentToolDispatcher>>,
                   default_source: Option<Arc<dyn AgentToolDispatcher>>,
                   per_spawn: Option<Arc<dyn AgentToolDispatcher>>| {
        super::super::tools::compose_external_tools_for_profile(
            profile,
            bundles,
            handle.clone(),
            default_source,
            per_spawn,
            None,
        )
        .expect("compose the member tools")
        .expect("a tools.mob member gets its operator tools")
    };
    let no_bundles = BTreeMap::new();
    let mut claimed_exact = Vec::new();

    // The mob-wide default source. `spawn_member` collides with an operator
    // tool, so the name filter wraps the source.
    let name_filter = compose(
        &worker_profile(&handle),
        &no_bundles,
        Some(inexact(&["host_alpha", "spawn_member"])),
        None,
    );
    if name_filter.tool_catalog_capabilities().exact_catalog {
        claimed_exact.push("name filter over the default source");
    }

    // The per-spawn source, which the name-order wrapper wraps; no name
    // collides, so no name filter is added.
    let name_order = compose(
        &worker_profile(&handle),
        &no_bundles,
        None,
        Some(inexact(&["host_beta"])),
    );
    if name_order.tool_catalog_capabilities().exact_catalog {
        claimed_exact.push("name order over the per-spawn source");
    }

    // A registered bundle, which the bundle provenance wrapper mounts.
    let mut bundle_profile = worker_profile(&handle);
    bundle_profile.tools.rust_bundles = vec!["reports".to_string()];
    let bundles = BTreeMap::from([("reports".to_string(), inexact(&["report_build"]))]);
    let bundle = compose(&bundle_profile, &bundles, None, None);
    if bundle.tool_catalog_capabilities().exact_catalog {
        claimed_exact.push("bundle provenance over a registered bundle");
    }

    // The MCP source filter over the default source; no name collides.
    let mut mcp_profile = worker_profile(&handle);
    mcp_profile.tools.mcp = vec!["allowed-server".to_string()];
    let mcp_filter = compose(
        &mcp_profile,
        &no_bundles,
        Some(inexact(&["host_gamma"])),
        None,
    );
    if mcp_filter.tool_catalog_capabilities().exact_catalog {
        claimed_exact.push("MCP source filter over the default source");
    }

    assert!(
        claimed_exact.is_empty(),
        "wrappers that claim an exact catalog over an inexact source: {claimed_exact:?}"
    );
}

/// The operator tools a placed member mounts on its own host, which forward
/// each call to the controlling host.
#[tokio::test]
async fn a_placed_members_operator_tool_catalog_is_exact_and_mirrors_its_tools() {
    let member_runtime = Arc::new(
        meerkat_comms::CommsRuntime::inproc_only(&format!(
            "operator-catalog-{}",
            Uuid::new_v4().simple()
        ))
        .expect("inproc comms runtime"),
    );
    let dispatcher = super::super::member_upcall::MemberUpcallToolDispatcher::new(
        AgentIdentity::from("placed-1"),
        Arc::new(super::super::member_upcall::MemberUpcallBindingStamp::new(
            1,
            1,
            "host-a",
            1,
            "member-session-a",
        )),
        meerkat_core::comms::PeerRoute::new(meerkat_core::comms::PeerId::from_ed25519_pubkey(
            &[9u8; 32],
        )),
        member_runtime,
        None,
    );
    assert!(
        dispatcher
            .tools()
            .iter()
            .any(|tool| tool.name == "spawn_member"),
        "the operator tools are listed"
    );
    assert_exact_inline_catalog(&dispatcher, "placed member operator tools");
}
