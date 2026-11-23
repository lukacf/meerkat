//! Rows for [`StockDeferralDispatcher`]: the re-tag rule condition by
//! condition, what passes through, and the forwarding census.

use super::*;
use crate::tool_catalog::{StockToolSourceKind, ToolCallability, ToolUnavailableReason};
use crate::types::{ToolResult, ToolSourceKind};
use crate::{ToolBindingDigest, ToolCatalogMode, ToolMutationClass};
use std::collections::BTreeSet;
use std::sync::Mutex;

type CallLog = Arc<Mutex<Vec<&'static str>>>;

fn provenance(kind: ToolSourceKind, source: &str) -> ToolProvenance {
    ToolProvenance {
        kind,
        source_id: source.into(),
        artifact: None,
    }
}

fn tool(name: &str, description: &str, provenance: Option<ToolProvenance>) -> Arc<ToolDef> {
    let mut tool = ToolDef::new(name, description, serde_json::json!({ "type": "object" }));
    tool.provenance = provenance;
    Arc::new(tool)
}

/// An inline session entry of a tool with provenance `kind`/`source`.
fn inline(name: &str, kind: ToolSourceKind, source: &str) -> ToolCatalogEntry {
    ToolCatalogEntry::session_inline(
        tool(name, "A small tool.", Some(provenance(kind, source))),
        true,
    )
}

fn comms(name: &str) -> ToolCatalogEntry {
    inline(name, ToolSourceKind::Comms, "comms")
}

/// A tool of the mob kind: a kind no policy can name.
fn mob(name: &str) -> ToolCatalogEntry {
    inline(name, ToolSourceKind::Mob, "mob")
}

fn comms_only() -> StockToolDeferralPolicy {
    StockToolDeferralPolicy::none().deferring(StockToolSourceKind::Comms)
}

const NO_PINS: [&str; 0] = [];

/// A tool source with an exact catalog that can change between reads, and
/// that records every dispatcher method called on it.
struct Source {
    catalog: Mutex<Vec<ToolCatalogEntry>>,
    /// Sources this one reports as still connecting (none unless a row adds one).
    pending: Mutex<Vec<String>>,
    calls: CallLog,
}

impl Source {
    fn new(catalog: Vec<ToolCatalogEntry>) -> Self {
        Self {
            catalog: Mutex::new(catalog),
            pending: Mutex::new(Vec::new()),
            calls: CallLog::default(),
        }
    }

    fn saw(&self, method: &'static str) {
        self.calls.lock().unwrap().push(method);
    }
}

#[async_trait]
impl AgentToolDispatcher for Source {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        self.saw("tools");
        self.catalog
            .lock()
            .unwrap()
            .iter()
            .map(|entry| Arc::clone(&entry.tool))
            .collect::<Vec<_>>()
            .into()
    }

    async fn resolve_tool_application(
        &self,
        source_tool: &str,
        _request: &crate::ToolApplicationRequest,
        _invocation: &serde_json::Value,
        _context: &ToolDispatchContext,
    ) -> Result<crate::tool_application::ToolApplicationResolution, ToolError> {
        self.saw("resolve_tool_application");
        Err(ToolError::not_found(source_tool))
    }

    fn tool_catalog_capabilities(&self) -> ToolCatalogCapabilities {
        self.saw("tool_catalog_capabilities");
        ToolCatalogCapabilities {
            exact_catalog: true,
            may_require_catalog_control_plane: false,
        }
    }

    fn tool_catalog(&self) -> Arc<[ToolCatalogEntry]> {
        self.saw("tool_catalog");
        self.catalog.lock().unwrap().clone().into()
    }

    fn tool_mutation_class(&self, _tool_name: &str) -> ToolMutationClass {
        self.saw("tool_mutation_class");
        ToolMutationClass::ReadOnly
    }

    fn live_bridge_effect_kind(&self, _tool_name: &str) -> crate::LiveBridgeEffectKind {
        self.saw("live_bridge_effect_kind");
        crate::LiveBridgeEffectKind::Comms
    }

    fn review_entry_support(
        &self,
        _tool_name: &str,
    ) -> crate::approval::review::ReviewEntrySupport {
        self.saw("review_entry_support");
        crate::approval::review::ReviewEntrySupport::ConsumesAtEntry
    }

    fn execution_binding_epoch(&self, _tool_name: &str) -> u64 {
        self.saw("execution_binding_epoch");
        7
    }

    fn execution_binding_fingerprint(
        &self,
        tool_name: &str,
    ) -> Result<crate::EphemeralToolBindingFingerprint, crate::ToolExecutionResolutionError> {
        self.saw("execution_binding_fingerprint");
        let catalog = self.catalog.lock().unwrap();
        let entry = catalog
            .iter()
            .find(|entry| entry.tool.name == tool_name)
            .ok_or_else(|| crate::ToolExecutionResolutionError::NotFound {
                tool_name: tool_name.to_string(),
            })?;
        Ok(crate::ephemeral_tool_catalog_binding_fingerprint(entry).with_live_authority(3, 7))
    }

    fn resolve_execution_plan(
        &self,
        call: ToolCallView<'_>,
        _dispatch_context: &ToolDispatchContext,
        resolution_context: &crate::ToolExecutionResolutionContext,
    ) -> Result<crate::ResolvedToolExecutionPlan, crate::ToolExecutionResolutionError> {
        self.saw("resolve_execution_plan");
        let catalog = self.catalog.lock().unwrap();
        let entry = catalog
            .iter()
            .find(|entry| entry.tool.name == call.name)
            .ok_or_else(|| crate::ToolExecutionResolutionError::NotFound {
                tool_name: call.name.to_string(),
            })?;
        entry
            .execution
            .resolve_default(resolution_context.deadlines().clone())
            .map_err(crate::ToolExecutionResolutionError::from)
    }

    fn validate_resolved_execution_plan(
        &self,
        _call: ToolCallView<'_>,
        resolution_context: &crate::ToolExecutionResolutionContext,
        plan: &crate::ResolvedToolExecutionPlan,
    ) -> Result<(), crate::ToolExecutionResolutionError> {
        self.saw("validate_resolved_execution_plan");
        resolution_context.validate_resolved_plan(plan)
    }

    fn pending_catalog_sources(&self) -> Arc<[String]> {
        self.saw("pending_catalog_sources");
        self.pending.lock().unwrap().clone().into()
    }

    fn adjudicates_tool_grants(&self) -> bool {
        self.saw("adjudicates_tool_grants");
        true
    }

    async fn dispatch(
        &self,
        call: ToolCallView<'_>,
    ) -> Result<crate::ops::ToolDispatchOutcome, ToolError> {
        self.saw("dispatch");
        Ok(ToolResult::new(call.id.to_string(), "plain".to_string(), false).into())
    }

    async fn dispatch_with_context(
        &self,
        call: ToolCallView<'_>,
        _context: &ToolDispatchContext,
    ) -> Result<crate::ops::ToolDispatchOutcome, ToolError> {
        self.saw("dispatch_with_context");
        Ok(ToolResult::new(call.id.to_string(), "with context".to_string(), false).into())
    }

    async fn dispatch_resolved_with_context(
        &self,
        call: ToolCallView<'_>,
        _context: &ToolDispatchContext,
        _plan: &crate::ResolvedToolExecutionPlan,
    ) -> Result<crate::ops::ToolDispatchOutcome, ToolError> {
        self.saw("dispatch_resolved_with_context");
        Ok(ToolResult::new(call.id.to_string(), "resolved".to_string(), false).into())
    }

    async fn poll_external_updates(&self) -> ExternalToolUpdate {
        self.saw("poll_external_updates");
        ExternalToolUpdate {
            notices: Vec::new(),
            pending: vec!["a-connecting-source".to_string()],
        }
    }

    fn external_tool_surface_snapshot(&self) -> Option<crate::ExternalToolSurfaceSnapshot> {
        self.saw("external_tool_surface_snapshot");
        None
    }

    fn capabilities(&self) -> DispatcherCapabilities {
        self.saw("capabilities");
        DispatcherCapabilities {
            ops_lifecycle: true,
        }
    }

    fn bind_ops_lifecycle(
        self: Arc<Self>,
        _registry: Arc<dyn crate::ops_lifecycle::OpsLifecycleRegistry>,
        _owner_bridge_session_id: crate::types::SessionId,
    ) -> Result<BindOutcome, OpsLifecycleBindError> {
        self.saw("bind_ops_lifecycle");
        Ok(BindOutcome::Bound(self))
    }

    fn completion_enrichment(
        &self,
    ) -> Option<Arc<dyn crate::completion_feed::CompletionEnrichmentProvider>> {
        self.saw("completion_enrichment");
        None
    }

    fn bind_session_mob_tool_authority(
        &self,
        _authority: Option<Arc<std::sync::RwLock<crate::service::MobToolAuthorityContext>>>,
    ) {
        self.saw("bind_session_mob_tool_authority");
    }

    fn observes_bindings_only(&self) -> bool {
        self.saw("observes_bindings_only");
        true
    }
}

fn wrap<const N: usize>(
    catalog: Vec<ToolCatalogEntry>,
    policy: StockToolDeferralPolicy,
    pinned: [&str; N],
) -> (StockDeferralDispatcher, Arc<Source>) {
    let source = Arc::new(Source::new(catalog));
    let inner: Arc<dyn AgentToolDispatcher> = source.clone();
    (StockDeferralDispatcher::new(inner, policy, pinned), source)
}

fn entry<'a>(catalog: &'a [ToolCatalogEntry], name: &str) -> &'a ToolCatalogEntry {
    catalog
        .iter()
        .find(|entry| entry.tool.name == name)
        .unwrap_or_else(|| panic!("no catalog entry named {name}"))
}

/// The names the catalog reports as deferred-eligible.
fn eligible(catalog: &[ToolCatalogEntry]) -> BTreeSet<String> {
    catalog
        .iter()
        .filter(|entry| {
            matches!(
                entry.deferred_eligibility,
                ToolCatalogDeferredEligibility::DeferredEligible { .. }
            )
        })
        .map(|entry| entry.tool.name.as_str().to_string())
        .collect()
}

fn names(names: &[&str]) -> BTreeSet<String> {
    names.iter().map(|name| (*name).to_string()).collect()
}

fn digests(catalog: &[ToolCatalogEntry]) -> Vec<(String, String)> {
    catalog
        .iter()
        .map(|entry| {
            (
                entry.tool.name.as_str().to_string(),
                ToolBindingDigest::of_catalog_entry(entry).to_string(),
            )
        })
        .collect()
}

/// Every provenance kind there is. The match is exhaustive, so a new kind
/// does not compile until someone decides here whether a stock deferral
/// policy can name it.
fn every_source_kind() -> Vec<ToolSourceKind> {
    fn nameable(kind: &ToolSourceKind) -> bool {
        match kind {
            ToolSourceKind::Comms => true,
            ToolSourceKind::Builtin
            | ToolSourceKind::Shell
            | ToolSourceKind::Memory
            | ToolSourceKind::Schedule
            | ToolSourceKind::WorkGraph
            | ToolSourceKind::Mob
            | ToolSourceKind::Callback
            | ToolSourceKind::Mcp
            | ToolSourceKind::RustBundle => false,
        }
    }
    let kinds = vec![
        ToolSourceKind::Builtin,
        ToolSourceKind::Shell,
        ToolSourceKind::Comms,
        ToolSourceKind::Memory,
        ToolSourceKind::Schedule,
        ToolSourceKind::WorkGraph,
        ToolSourceKind::Mob,
        ToolSourceKind::Callback,
        ToolSourceKind::Mcp,
        ToolSourceKind::RustBundle,
    ];
    assert_eq!(
        kinds.iter().filter(|kind| nameable(kind)).count(),
        1,
        "one kind can be named"
    );
    kinds
}

/// The policy's vocabulary: the default names nothing, a family is named
/// only when it was given, and of every provenance kind only the comms kind
/// can be named at all.
#[test]
fn the_policy_names_only_the_families_it_was_given() {
    let none = StockToolDeferralPolicy::none();
    assert!(none.is_empty());
    assert_eq!(none, StockToolDeferralPolicy::default());
    assert!(!none.allows(StockToolSourceKind::Comms));

    let comms = comms_only();
    assert!(!comms.is_empty());
    assert!(comms.allows(StockToolSourceKind::Comms));
    assert_eq!(
        comms,
        comms_only().deferring(StockToolSourceKind::Comms),
        "naming a family twice names it once"
    );

    assert_eq!(
        StockToolSourceKind::Comms.source_kind(),
        ToolSourceKind::Comms
    );

    for kind in every_source_kind() {
        let expected = matches!(kind, ToolSourceKind::Comms);
        assert_eq!(
            comms.allows_source_kind(&kind),
            expected,
            "the policy naming every family and {kind:?}"
        );
        assert!(
            !none.allows_source_kind(&kind),
            "the empty policy names no kind: {kind:?}"
        );
    }
}

/// With the empty policy the catalog is the wrapped
/// dispatcher's own, entry for entry and digest for digest.
#[test]
fn an_empty_policy_changes_nothing() {
    let catalog = vec![
        comms("send_message"),
        comms("reply_to_peer"),
        mob("mob_spawn_member"),
        inline("read_file", ToolSourceKind::Builtin, "builtin"),
    ];
    let (wrapper, source) = wrap(catalog, StockToolDeferralPolicy::none(), ["reply_to_peer"]);
    let inner = source.tool_catalog();
    let wrapped = wrapper.tool_catalog();
    assert_eq!(digests(&wrapped), digests(&inner));
    assert!(eligible(&wrapped).is_empty());
    assert!(
        wrapped
            .iter()
            .zip(inner.iter())
            .all(|(wrapped, inner)| Arc::ptr_eq(&wrapped.tool, &inner.tool)),
        "the tool definitions are the wrapped dispatcher's own"
    );
}

/// A named family's inline session entries become deferred-eligible, each
/// owned by its tool's own provenance (two sources of the comms kind keep
/// their own source ids). A tool of a kind no policy names stays as it is.
#[test]
fn a_named_familys_tools_are_re_tagged_with_their_own_provenance() {
    let (wrapper, _source) = wrap(
        vec![
            comms("send_message"),
            inline("relay_message", ToolSourceKind::Comms, "comms-relay"),
            mob("mob_spawn_member"),
        ],
        comms_only(),
        NO_PINS,
    );
    let catalog = wrapper.tool_catalog();
    for (name, owner) in [
        ("send_message", provenance(ToolSourceKind::Comms, "comms")),
        (
            "relay_message",
            provenance(ToolSourceKind::Comms, "comms-relay"),
        ),
    ] {
        let entry = entry(&catalog, name);
        assert_eq!(
            entry.deferred_eligibility,
            ToolCatalogDeferredEligibility::DeferredEligible {
                provenance: owner.clone()
            },
            "{name} is deferred-eligible"
        );
        assert_eq!(
            entry.tool.provenance.as_ref(),
            Some(&owner),
            "{name} is owned behind the catalog by its tool's own provenance"
        );
    }
    assert_eq!(
        entry(&catalog, "mob_spawn_member").deferred_eligibility,
        ToolCatalogDeferredEligibility::InlineOnly,
        "a mob tool is of no family the policy names"
    );
}

/// The comms policy re-tags the comms tools that are
/// not pinned, and no tool of another kind.
#[test]
fn comms_policy_defers_only_unpinned_comms_tools() {
    let (wrapper, _source) = wrap(
        vec![
            comms("send_message"),
            comms("reply_to_peer"),
            comms("send_request"),
            comms("send_response"),
            comms("peers"),
            mob("mob_spawn_member"),
            inline("read_file", ToolSourceKind::Builtin, "builtin"),
            inline("memory_search", ToolSourceKind::Memory, "memory"),
            inline("host_lookup", ToolSourceKind::Callback, "host"),
        ],
        comms_only(),
        ["reply_to_peer", "send_response"],
    );
    let catalog = wrapper.tool_catalog();
    assert_eq!(
        eligible(&catalog),
        names(&["send_message", "send_request", "peers"])
    );
    for name in ["send_message", "send_request", "peers"] {
        assert_eq!(
            entry(&catalog, name).deferred_eligibility,
            ToolCatalogDeferredEligibility::DeferredEligible {
                provenance: provenance(ToolSourceKind::Comms, "comms")
            }
        );
    }
}

/// A tool whose execution contract is not the default one stays inline
/// under a policy that names its family, with the contract and the digest
/// its source declares; the family's tools with the default contract wait.
/// No comms tool declares such a contract today, so the tool here is a
/// stand-in of the comms kind.
#[test]
fn a_tool_that_owns_its_deadline_is_never_re_tagged() {
    let owns_deadline = ToolExecutionContract::default().with_tool_owned_deadline();
    let (wrapper, source) = wrap(
        vec![
            comms("await_reply").with_execution_contract(owns_deadline.clone()),
            comms("send_message"),
            comms("peers"),
            mob("mob_spawn_member"),
        ],
        comms_only(),
        NO_PINS,
    );
    let inner = source.tool_catalog();
    let catalog = wrapper.tool_catalog();
    let wrapped = entry(&catalog, "await_reply");
    assert_eq!(
        wrapped.deferred_eligibility,
        ToolCatalogDeferredEligibility::InlineOnly,
        "a tool that owns its deadline stays in every request"
    );
    assert_eq!(wrapped.execution, owns_deadline, "it keeps its contract");
    assert_eq!(
        ToolBindingDigest::of_catalog_entry(wrapped),
        ToolBindingDigest::of_catalog_entry(entry(&inner, "await_reply")),
        "it keeps its declaration digest"
    );
    assert_eq!(eligible(&catalog), names(&["send_message", "peers"]));
}

/// The policy judges a tool by its own provenance. A host tool that won
/// a comms tool's name is not a comms tool, and an entry a host already made
/// deferred-eligible keeps the owner its source gave it.
#[test]
fn a_host_tool_that_wins_a_stock_name_is_judged_by_its_own_provenance() {
    let host = provenance(ToolSourceKind::Callback, "host");
    let (wrapper, _source) = wrap(
        vec![
            inline("send_message", ToolSourceKind::Callback, "host"),
            ToolCatalogEntry::session_deferred(
                tool("peers", "A host tool.", Some(host.clone())),
                true,
                host.clone(),
            ),
            comms("send_request"),
        ],
        comms_only(),
        NO_PINS,
    );
    let catalog = wrapper.tool_catalog();
    assert_eq!(
        entry(&catalog, "send_message").deferred_eligibility,
        ToolCatalogDeferredEligibility::InlineOnly,
        "a host tool named send_message is the host's inline tool"
    );
    assert_eq!(
        entry(&catalog, "peers").deferred_eligibility,
        ToolCatalogDeferredEligibility::DeferredEligible { provenance: host },
        "a host tool named peers keeps its host owner"
    );
    assert_eq!(
        entry(&catalog, "send_request").deferred_eligibility,
        ToolCatalogDeferredEligibility::DeferredEligible {
            provenance: provenance(ToolSourceKind::Comms, "comms")
        },
        "the comms tool beside them waits"
    );
}

/// The entries the rule leaves alone although a named family's provenance is
/// on them: a control-plane entry, an entry already deferred-eligible under
/// another owner, and beside them a tool with no provenance at all.
#[test]
fn only_inline_session_entries_with_provenance_are_re_tagged() {
    let comms_provenance = provenance(ToolSourceKind::Comms, "comms");
    let other_owner = provenance(ToolSourceKind::Comms, "another-comms-source");
    let (wrapper, _source) = wrap(
        vec![
            ToolCatalogEntry::control_inline(
                tool(
                    "comms_control",
                    "A control tool.",
                    Some(comms_provenance.clone()),
                ),
                true,
            ),
            ToolCatalogEntry::session_deferred(
                tool(
                    "already_deferred",
                    "A deferred tool.",
                    Some(comms_provenance.clone()),
                ),
                true,
                other_owner.clone(),
            ),
            ToolCatalogEntry::session_inline(tool("unattributed", "No provenance.", None), true),
            comms("send_message"),
        ],
        comms_only(),
        NO_PINS,
    );
    let catalog = wrapper.tool_catalog();
    assert_eq!(
        entry(&catalog, "comms_control").deferred_eligibility,
        ToolCatalogDeferredEligibility::InlineOnly,
        "a control-plane entry is never deferred"
    );
    assert_eq!(
        entry(&catalog, "comms_control").plane,
        ToolPlaneClass::Control
    );
    assert_eq!(
        entry(&catalog, "already_deferred").deferred_eligibility,
        ToolCatalogDeferredEligibility::DeferredEligible {
            provenance: other_owner
        },
        "an entry its source made deferred-eligible keeps that owner"
    );
    assert_eq!(
        entry(&catalog, "unattributed").deferred_eligibility,
        ToolCatalogDeferredEligibility::InlineOnly,
        "a tool without provenance belongs to no family"
    );
    assert_eq!(
        entry(&catalog, "send_message").deferred_eligibility,
        ToolCatalogDeferredEligibility::DeferredEligible {
            provenance: comms_provenance
        },
        "the inline session comms tool beside them waits"
    );
}

/// A re-tagged entry differs from the wrapped one in its deferred
/// eligibility only: the tool definition, plane, callability and execution
/// contract are the wrapped dispatcher's, and its digest is the digest of
/// that same declaration made deferred-eligible.
#[test]
fn a_re_tagged_entry_keeps_its_tool_callability_contract_and_plane() {
    let unavailable =
        ToolCallability::unavailable(ToolUnavailableReason::RuntimeCommandAuthorityUnavailable);
    let request = ToolCatalogEntry::session_inline_with_callability(
        tool(
            "send_request",
            "A comms tool.",
            Some(provenance(ToolSourceKind::Comms, "comms")),
        ),
        unavailable,
    );
    let (wrapper, source) = wrap(vec![request, comms("send_message")], comms_only(), NO_PINS);
    let inner = source.tool_catalog();
    let catalog = wrapper.tool_catalog();
    assert_eq!(catalog.len(), inner.len());
    for (wrapped, inner) in catalog.iter().zip(inner.iter()) {
        assert!(Arc::ptr_eq(&wrapped.tool, &inner.tool));
        assert_eq!(wrapped.plane, inner.plane);
        assert_eq!(wrapped.callability, inner.callability);
        assert_eq!(wrapped.execution, inner.execution);
        let as_deferred = ToolCatalogEntry::session_deferred_with_callability(
            Arc::clone(&inner.tool),
            inner.callability,
            provenance(ToolSourceKind::Comms, "comms"),
        );
        assert_eq!(
            ToolBindingDigest::of_catalog_entry(wrapped),
            ToolBindingDigest::of_catalog_entry(&as_deferred),
            "{}: the digest of the same declaration, deferred-eligible",
            wrapped.tool.name
        );
        assert_ne!(
            ToolBindingDigest::of_catalog_entry(wrapped),
            ToolBindingDigest::of_catalog_entry(inner),
            "{}: not the inline form's digest",
            wrapped.tool.name
        );
    }
    assert_eq!(entry(&catalog, "send_request").callability, unavailable);
}

/// The catalog is read from the wrapped dispatcher on every call: a tool
/// that becomes callable there is callable in the next read, still
/// deferred-eligible.
#[test]
fn callability_follows_the_wrapped_dispatcher_on_every_read() {
    let unavailable =
        ToolCallability::unavailable(ToolUnavailableReason::RuntimeCommandAuthorityUnavailable);
    let (wrapper, source) = wrap(
        vec![ToolCatalogEntry::session_inline_with_callability(
            tool(
                "send_request",
                "A comms tool.",
                Some(provenance(ToolSourceKind::Comms, "comms")),
            ),
            unavailable,
        )],
        comms_only(),
        NO_PINS,
    );
    assert_eq!(
        entry(&wrapper.tool_catalog(), "send_request").callability,
        unavailable
    );
    source.catalog.lock().unwrap()[0].callability = ToolCallability::callable();
    let catalog = wrapper.tool_catalog();
    assert_eq!(
        entry(&catalog, "send_request").callability,
        ToolCallability::callable()
    );
    assert_eq!(eligible(&catalog), names(&["send_request"]));
}

/// The wrapper's binding for a tool is its own entry (the re-tagged one)
/// with the wrapped dispatcher's binding as its dependency.
#[test]
fn the_binding_fingerprint_is_the_re_tagged_entry_over_the_wrapped_binding() {
    let (wrapper, source) = wrap(
        vec![comms("send_message"), comms("reply_to_peer")],
        comms_only(),
        ["reply_to_peer"],
    );
    let catalog = wrapper.tool_catalog();
    for name in ["send_message", "reply_to_peer"] {
        let expected = crate::ephemeral_tool_catalog_binding_fingerprint(entry(&catalog, name))
            .with_live_authority(0, 0)
            .with_dependency(&source.execution_binding_fingerprint(name).unwrap());
        let fingerprint = wrapper.execution_binding_fingerprint(name).unwrap();
        assert_eq!(fingerprint, expected, "{name}");
        assert_eq!(
            fingerprint.declaration_digest(),
            ToolBindingDigest::of_catalog_entry(entry(&catalog, name)),
            "{name}: the binding declares what the catalog reports"
        );
    }
    assert_eq!(
        wrapper
            .execution_binding_fingerprint("send_message")
            .unwrap()
            .declaration_digest(),
        ToolBindingDigest::of_catalog_entry(&ToolCatalogEntry::session_deferred(
            Arc::clone(&entry(&catalog, "send_message").tool),
            true,
            provenance(ToolSourceKind::Comms, "comms"),
        )),
        "send_message is bound as a deferred-eligible tool"
    );
    assert!(matches!(
        wrapper.execution_binding_fingerprint("absent"),
        Err(crate::ToolExecutionResolutionError::NotFound { .. })
    ));
}

/// A tool of about `bytes` bytes of definition text.
fn sized(name: &str, kind: ToolSourceKind, source: &str, bytes: usize) -> ToolCatalogEntry {
    ToolCatalogEntry::session_inline(
        tool(name, &"d".repeat(bytes), Some(provenance(kind, source))),
        true,
    )
}

/// Re-tagged stock tools select the session's catalog mode by the ordinary
/// rule, like any deferred-eligible tool: an exact catalog defers from
/// `DEFERRED_CATALOG_TOOL_COUNT_THRESHOLD` deferred-eligible tools or
/// `DEFERRED_CATALOG_SCHEMA_VOLUME_THRESHOLD` bytes of their definitions.
/// Below both a re-tagged tool stays in the request, while the control
/// plane is still composed for it.
#[test]
fn re_tagged_tools_select_the_deferred_mode_by_count_or_volume() {
    use crate::tool_catalog::{
        DEFERRED_CATALOG_SCHEMA_VOLUME_THRESHOLD, DEFERRED_CATALOG_TOOL_COUNT_THRESHOLD,
    };

    // By count: small tools, as many as the count threshold.
    let (by_count, by_count_source) = wrap(
        vec![
            comms("send_message"),
            comms("send_request"),
            comms("reply_to_peer"),
        ],
        comms_only(),
        ["reply_to_peer"],
    );
    assert_eq!(
        crate::select_tool_catalog_mode(by_count_source.as_ref()),
        ToolCatalogMode::Inline,
        "precondition: without the policy nothing is deferred-eligible"
    );
    assert!(!crate::should_compose_tool_catalog_control_plane(
        by_count_source.as_ref()
    ));
    let catalog = by_count.tool_catalog();
    assert_eq!(
        crate::deferred_session_entry_count(&catalog),
        DEFERRED_CATALOG_TOOL_COUNT_THRESHOLD,
        "precondition: the re-tagged tools reach the count"
    );
    assert!(
        crate::deferred_session_schema_volume(&catalog) < DEFERRED_CATALOG_SCHEMA_VOLUME_THRESHOLD,
        "precondition: and not the volume"
    );
    assert_eq!(
        crate::select_tool_catalog_mode(&by_count),
        ToolCatalogMode::Deferred
    );
    assert!(crate::should_compose_tool_catalog_control_plane(&by_count));

    // By volume: one tool whose definition is the volume threshold's size.
    let (by_volume, _source) = wrap(
        vec![
            sized(
                "send_message",
                ToolSourceKind::Comms,
                "comms",
                DEFERRED_CATALOG_SCHEMA_VOLUME_THRESHOLD,
            ),
            comms("reply_to_peer"),
        ],
        comms_only(),
        ["reply_to_peer"],
    );
    assert_eq!(
        crate::deferred_session_entry_count(&by_volume.tool_catalog()),
        1,
        "precondition: one re-tagged tool is below the count"
    );
    assert_eq!(
        crate::select_tool_catalog_mode(&by_volume),
        ToolCatalogMode::Deferred
    );

    // Below both: one small tool.
    let (below, _source) = wrap(
        vec![comms("send_message"), comms("reply_to_peer")],
        comms_only(),
        ["reply_to_peer"],
    );
    assert_eq!(eligible(&below.tool_catalog()), names(&["send_message"]));
    assert_eq!(
        crate::select_tool_catalog_mode(&below),
        ToolCatalogMode::Inline,
        "one small tool stays in the request"
    );
    assert!(
        crate::should_compose_tool_catalog_control_plane(&below),
        "the control plane is composed for a tool that may wait later"
    );
}

/// A source a host composes over the wrapper lists the wrapper's entries as
/// the wrapper reports them, and its own entries are not the policy's to
/// re-tag.
#[test]
fn a_gateway_over_the_wrapper_passes_the_re_tagged_entries_through() {
    let (wrapper, _source) = wrap(
        vec![comms("send_message"), comms("reply_to_peer")],
        comms_only(),
        ["reply_to_peer"],
    );
    let wrapped_digests = digests(&wrapper.tool_catalog());
    let later = Source::new(vec![inline(
        "memory_search",
        ToolSourceKind::Memory,
        "memory",
    )]);
    let gateway = crate::ToolGatewayBuilder::new()
        .add_dispatcher(Arc::new(wrapper))
        .add_dispatcher(Arc::new(later))
        .build()
        .expect("a gateway over the wrapper and a later source");
    let catalog = gateway.tool_catalog();
    for (name, digest) in wrapped_digests {
        assert_eq!(
            ToolBindingDigest::of_catalog_entry(entry(&catalog, &name)).to_string(),
            digest,
            "{name} is listed as the wrapper reports it"
        );
    }
    assert_eq!(eligible(&catalog), names(&["send_message"]));
    assert_eq!(
        entry(&catalog, "memory_search").deferred_eligibility,
        ToolCatalogDeferredEligibility::InlineOnly,
        "a source composed after the wrapper is outside the policy"
    );
}

fn call<'a>(name: &'a str, args: &'a serde_json::value::RawValue) -> ToolCallView<'a> {
    ToolCallView {
        id: "call-1",
        name,
        args,
    }
}

/// Each call on the wrapper reaches the wrapped
/// dispatcher and returns its answer. The two handle binds are covered by
/// the source census below.
#[tokio::test]
async fn every_forwarded_call_reaches_the_wrapped_dispatcher() {
    let (wrapper, source) = wrap(vec![comms("send_message")], comms_only(), NO_PINS);
    source
        .pending
        .lock()
        .unwrap()
        .push("a-connecting-source".to_string());
    let args = serde_json::value::RawValue::from_string("{}".to_string()).unwrap();
    let context = ToolDispatchContext::default();
    let resolution = crate::ToolExecutionResolutionContext::new(
        crate::ToolDeadlineChain::new(vec![crate::ToolDeadlineContributor::finite(
            crate::ToolDeadlineOwner::CoreToolDispatch,
            std::time::Duration::from_mins(10),
        )])
        .unwrap(),
    );

    assert_eq!(wrapper.tools().len(), 1);
    assert!(wrapper.tool_catalog_capabilities().exact_catalog);
    assert_eq!(wrapper.tool_catalog().len(), 1);
    assert_eq!(
        wrapper.tool_mutation_class("send_message"),
        ToolMutationClass::ReadOnly
    );
    assert_eq!(
        wrapper.live_bridge_effect_kind("send_message"),
        crate::LiveBridgeEffectKind::Comms
    );
    assert_eq!(
        wrapper.review_entry_support("send_message"),
        crate::approval::review::ReviewEntrySupport::ConsumesAtEntry
    );
    assert_eq!(wrapper.execution_binding_epoch("send_message"), 7);
    assert!(
        wrapper
            .execution_binding_fingerprint("send_message")
            .is_ok()
    );
    assert_eq!(
        wrapper.pending_catalog_sources().as_ref(),
        ["a-connecting-source".to_string()]
    );
    assert!(wrapper.adjudicates_tool_grants());
    assert!(wrapper.observes_bindings_only());
    assert!(wrapper.capabilities().ops_lifecycle);
    assert!(wrapper.external_tool_surface_snapshot().is_none());
    assert!(wrapper.completion_enrichment().is_none());
    wrapper.bind_session_mob_tool_authority(None);
    assert_eq!(
        wrapper.poll_external_updates().await.pending,
        ["a-connecting-source".to_string()]
    );

    let view = call("send_message", &args);
    assert!(matches!(
        wrapper
            .resolve_tool_application(
                "send_message",
                &crate::ToolApplicationRequest {
                    tool_call_id: "call-1".into(),
                    extension: "example.test/app".into(),
                    operation: crate::ToolApplicationOperation::Resolve,
                },
                &serde_json::Value::Null,
                &context,
            )
            .await,
        Err(ToolError::NotFound { .. })
    ));
    let plan = wrapper
        .resolve_execution_plan(view, &context, &resolution)
        .expect("the wrapped dispatcher's plan");
    wrapper
        .validate_resolved_execution_plan(view, &resolution, &plan)
        .expect("the wrapped dispatcher validates its plan");
    assert_eq!(
        wrapper.dispatch(view).await.unwrap().result.text_content(),
        "plain"
    );
    assert_eq!(
        wrapper
            .dispatch_with_context(view, &context)
            .await
            .unwrap()
            .result
            .text_content(),
        "with context"
    );
    assert_eq!(
        wrapper
            .dispatch_resolved_with_context(view, &context, &plan)
            .await
            .unwrap()
            .result
            .text_content(),
        "resolved"
    );

    let calls = Arc::clone(&source.calls);
    drop(source);
    let bound = Arc::new(wrapper)
        .bind_ops_lifecycle(
            Arc::new(crate::tool_execution_policy::tests::UnsupportedOpsRegistry),
            crate::types::SessionId::new(),
        )
        .expect("the bind reaches the wrapped dispatcher");
    assert!(bound.was_bound());
    assert_eq!(
        eligible(&bound.into_dispatcher().tool_catalog()),
        names(&["send_message"]),
        "the rebound dispatcher still applies the policy"
    );

    let seen = calls
        .lock()
        .unwrap()
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();
    let expected = [
        "tools",
        "resolve_tool_application",
        "tool_catalog_capabilities",
        "tool_catalog",
        "tool_mutation_class",
        "live_bridge_effect_kind",
        "review_entry_support",
        "execution_binding_epoch",
        "execution_binding_fingerprint",
        "resolve_execution_plan",
        "validate_resolved_execution_plan",
        "pending_catalog_sources",
        "adjudicates_tool_grants",
        "dispatch",
        "dispatch_with_context",
        "dispatch_resolved_with_context",
        "poll_external_updates",
        "external_tool_surface_snapshot",
        "capabilities",
        "bind_ops_lifecycle",
        "completion_enrichment",
        "bind_session_mob_tool_authority",
        "observes_bindings_only",
    ]
    .into_iter()
    .collect::<BTreeSet<_>>();
    assert_eq!(seen, expected);
}

/// A shared wrapper, or a wrapper over a shared dispatcher that needs the
/// binding, cannot be rebound: the bind is refused, never skipped silently.
#[test]
fn a_shared_wrapper_refuses_the_ops_lifecycle_bind() {
    let registry = || {
        Arc::new(crate::tool_execution_policy::tests::UnsupportedOpsRegistry)
            as Arc<dyn crate::ops_lifecycle::OpsLifecycleRegistry>
    };
    let (wrapper, _source) = wrap(vec![comms("send_message")], comms_only(), NO_PINS);
    let shared = Arc::new(wrapper);
    let second_owner = Arc::clone(&shared);
    assert!(matches!(
        shared.bind_ops_lifecycle(registry(), crate::types::SessionId::new()),
        Err(OpsLifecycleBindError::SharedOwnership)
    ));
    drop(second_owner);

    let (wrapper, source) = wrap(vec![comms("send_message")], comms_only(), NO_PINS);
    assert!(matches!(
        Arc::new(wrapper).bind_ops_lifecycle(registry(), crate::types::SessionId::new()),
        Err(OpsLifecycleBindError::SharedOwnership)
    ));
    assert!(
        !source.calls.lock().unwrap().contains(&"bind_ops_lifecycle"),
        "a shared wrapped dispatcher is not bound behind its other owner"
    );
}

/// Rust source with comments and string literals blanked, so braces and
/// `fn` in them are not read as code.
fn code_only(source: &str) -> String {
    let mut code = String::with_capacity(source.len());
    let mut chars = source.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '/' if chars.peek() == Some(&'/') => {
                for skipped in chars.by_ref() {
                    if skipped == '\n' {
                        code.push('\n');
                        break;
                    }
                }
            }
            '/' if chars.peek() == Some(&'*') => {
                chars.next();
                let mut previous = ' ';
                for skipped in chars.by_ref() {
                    if previous == '*' && skipped == '/' {
                        break;
                    }
                    previous = skipped;
                }
            }
            '"' => {
                code.push('"');
                while let Some(skipped) = chars.next() {
                    match skipped {
                        '\\' => {
                            chars.next();
                        }
                        '"' => break,
                        _ => {}
                    }
                }
                code.push('"');
            }
            other => code.push(other),
        }
    }
    code
}

/// The methods declared directly in the block that `header` opens in
/// `source`: each name with its body (empty for a method without one),
/// whitespace removed.
fn methods_of_block(source: &str, header: &str) -> Vec<(String, String)> {
    let start = source
        .find(header)
        .unwrap_or_else(|| panic!("no `{header}` in the source"));
    let code = code_only(&source[start..]);
    let open = code.find('{').expect("the block opens");
    let mut methods = Vec::new();
    let mut depth = 0_usize;
    let mut index = open;
    let bytes = code.as_bytes();
    while index < bytes.len() {
        match bytes[index] {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return methods;
                }
            }
            b'f' if depth == 1
                && code[index..].starts_with("fn ")
                && bytes[index - 1].is_ascii_whitespace() =>
            {
                let name_start = index + 3;
                let name_len = code[name_start..]
                    .find(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_'))
                    .expect("a method name ends");
                let name = code[name_start..name_start + name_len].to_string();
                let after_name = name_start + name_len;
                let end = code[after_name..]
                    .find(['{', ';'])
                    .expect("a method has a body or ends")
                    + after_name;
                if bytes[end] == b';' {
                    methods.push((name, String::new()));
                    index = end;
                } else {
                    let mut body_depth = 0_usize;
                    let mut cursor = end;
                    loop {
                        match bytes[cursor] {
                            b'{' => body_depth += 1,
                            b'}' => {
                                body_depth -= 1;
                                if body_depth == 0 {
                                    break;
                                }
                            }
                            _ => {}
                        }
                        cursor += 1;
                    }
                    let body = code[end..=cursor]
                        .chars()
                        .filter(|ch| !ch.is_whitespace())
                        .collect();
                    methods.push((name, body));
                    index = cursor;
                }
            }
            _ => {}
        }
        index += 1;
    }
    panic!("the block that `{header}` opens does not close");
}

/// The census: the wrapper overrides every method of
/// [`AgentToolDispatcher`] (so none falls back to a trait default that would
/// answer for the wrapper instead of the wrapped dispatcher), and each
/// override calls the wrapped dispatcher's method of the same name. A method
/// added to the trait fails this row until the wrapper forwards it.
#[test]
fn the_wrapper_overrides_every_dispatcher_method_and_forwards_it() {
    let declared = methods_of_block(include_str!("../agent.rs"), "pub trait AgentToolDispatcher")
        .into_iter()
        .map(|(name, _)| name)
        .collect::<BTreeSet<_>>();
    for known in [
        "tools",
        "tool_catalog",
        "tool_catalog_capabilities",
        "dispatch",
        "bind_session_mob_tool_authority",
        "observes_bindings_only",
    ] {
        assert!(
            declared.contains(known),
            "the census reads the trait: {known} is one of its methods, found {declared:?}"
        );
    }
    assert!(
        declared.len() >= 25,
        "the trait had 25 methods when this row was written, found {}",
        declared.len()
    );

    let overridden = methods_of_block(
        include_str!("../stock_tool_deferral.rs"),
        "impl AgentToolDispatcher for StockDeferralDispatcher",
    );
    let overridden_names = overridden
        .iter()
        .map(|(name, _)| name.clone())
        .collect::<BTreeSet<_>>();
    assert_eq!(
        overridden_names, declared,
        "the wrapper overrides exactly the trait's methods"
    );
    for (name, body) in &overridden {
        assert!(
            body.contains(&format!("inner.{name}(")),
            "the wrapper's {name} calls the wrapped dispatcher's {name}: {body}"
        );
    }
}
