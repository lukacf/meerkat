//! Declared-versus-resolved member tool capability invariant.
//!
//! The declared side is the complete set of typed
//! [`meerkat_core::ToolCategoryOverride`] values on the member's resolved
//! [`meerkat_core::service::SessionBuildOptions`]. That boundary is after
//! inline or realm-profile resolution and before the factory consumes the
//! overrides. MCP source selectors and named Rust bundles are intentionally
//! outside this closed set: they are composition inputs, not typed category
//! overrides, and this invariant never invents declaration intent for them.
//!
//! The resolved side is read from the exact live session's tool-scope catalog
//! after `create_session` returns. A non-exact dispatcher chain is not evidence
//! of absence and therefore produces [`CapabilityInvariantDecision::Unverifiable`],
//! never a match or a gap.
//!
//! Reporting follows the decision, not the other way round
//! ([`CapabilityInvariantReport`]): a verified [`CapabilityInvariantDecision::Gap`]
//! is the only WarnOnly outcome that asks for operator action. An
//! `Unverifiable` outcome is a steady state for any host whose build supplies a
//! dispatcher without an exact catalog; it is explained once per process per
//! cause at INFO and then recorded per member at DEBUG. A failed catalog read
//! is an error and stays at WARN, but it is reported as a read failure, not as
//! a capability mismatch.

use std::collections::BTreeSet;

use meerkat_core::ToolCategoryOverride;
use meerkat_core::service::SessionBuildOptions;

/// The closed set of factory-owned tool categories a profile may declare.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DeclaredToolCategory {
    Builtins,
    Shell,
    Comms,
    Memory,
    Schedule,
    WorkGraph,
    Mob,
    ImageGeneration,
    WebSearch,
}

/// First-party mob tool names that the `mob_` prefix does not cover.
///
/// This is a PRESENCE classifier: "does this live catalog prove the mob
/// capability surface is wired?" It is deliberately NOT the same set as
/// `console_spawn::MOB_SPAWN_TOOL_VOCABULARY`, which answers a different
/// question ("did this tool call create a member the console must render?").
/// Every retire/wire/status name below proves the surface is present but
/// spawns nothing, and the two sets are free to diverge further. Do not
/// unify them.
///
/// Widening this list only ever removes false `Gap` decisions, so a name that
/// is genuinely part of the mob surface belongs here even when it co-occurs
/// with a `mob_`-prefixed sibling in every catalog seen so far.
pub(crate) const MOB_UNPREFIXED_TOOL_NAMES: &[&str] = &[
    "spawn_member",
    "spawn_many_members",
    "retire_member",
    "wire_members",
    "unwire_members",
    "list_members",
    "member_status",
    "force_cancel_member",
    "fork_off",
    "delegate",
];

impl DeclaredToolCategory {
    pub const ALL: [Self; 9] = [
        Self::Builtins,
        Self::Shell,
        Self::Comms,
        Self::Memory,
        Self::Schedule,
        Self::WorkGraph,
        Self::Mob,
        Self::ImageGeneration,
        Self::WebSearch,
    ];

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Builtins => "builtins",
            Self::Shell => "shell",
            Self::Comms => "comms",
            Self::Memory => "memory",
            Self::Schedule => "schedule",
            Self::WorkGraph => "workgraph",
            Self::Mob => "mob",
            Self::ImageGeneration => "image_generation",
            Self::WebSearch => "web_search",
        }
    }

    /// Whether an exact live catalog contains this category's canonical
    /// first-party surface. This classifies catalog truth; it does not inspect
    /// configuration or infer capability from a requested flag.
    #[must_use]
    pub fn is_present_in(self, names: &BTreeSet<String>) -> bool {
        let contains = |name: &str| names.contains(name);
        match self {
            Self::Builtins => names.iter().any(|name| {
                matches!(
                    name.as_str(),
                    "task_create"
                        | "task_get"
                        | "task_list"
                        | "task_update"
                        | "apply_patch"
                        | "datetime"
                        | "view_image"
                        | "browse_skills"
                        | "load_skill"
                ) || name.starts_with("blob_")
            }),
            Self::Shell => names.iter().any(|name| {
                name == "shell"
                    || name == "monitor_start"
                    || name.starts_with("shell_job")
                    || name == "shell_jobs"
            }),
            Self::Comms => contains("send_message") || contains("peers"),
            Self::Memory => contains("memory_search"),
            Self::Schedule => names
                .iter()
                .any(|name| name.starts_with("meerkat_schedule_")),
            Self::WorkGraph => names.iter().any(|name| name.starts_with("workgraph_")),
            Self::Mob => names.iter().any(|name| {
                MOB_UNPREFIXED_TOOL_NAMES.contains(&name.as_str()) || name.starts_with("mob_")
            }),
            Self::ImageGeneration => contains("generate_image"),
            Self::WebSearch => contains("web_search"),
        }
    }
}

impl std::fmt::Display for DeclaredToolCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Complete explicit category intent captured before the factory consumes it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DeclaredToolCategories(BTreeSet<DeclaredToolCategory>);

impl DeclaredToolCategories {
    #[must_use]
    pub fn from_build_options(build: &SessionBuildOptions) -> Self {
        let declared = [
            (DeclaredToolCategory::Builtins, build.override_builtins),
            (DeclaredToolCategory::Shell, build.override_shell),
            (DeclaredToolCategory::Comms, build.override_comms),
            (DeclaredToolCategory::Memory, build.override_memory),
            (DeclaredToolCategory::Schedule, build.override_schedule),
            (DeclaredToolCategory::WorkGraph, build.override_workgraph),
            (DeclaredToolCategory::Mob, build.override_mob),
            (
                DeclaredToolCategory::ImageGeneration,
                build.override_image_generation,
            ),
            (DeclaredToolCategory::WebSearch, build.override_web_search),
        ]
        .into_iter()
        .filter_map(|(category, intent)| {
            matches!(intent, ToolCategoryOverride::Enable).then_some(category)
        })
        .collect();
        Self(declared)
    }

    pub fn iter(&self) -> impl Iterator<Item = DeclaredToolCategory> + '_ {
        self.0.iter().copied()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl std::fmt::Display for DeclaredToolCategories {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.0.is_empty() {
            return f.write_str("none");
        }
        for (index, category) in self.0.iter().enumerate() {
            if index > 0 {
                f.write_str(",")?;
            }
            f.write_str(category.as_str())?;
        }
        Ok(())
    }
}

/// Per-member declaration and catalog-completeness witness captured from the
/// fully resolved build request before the factory consumes its dispatchers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemberCapabilityInvariantContext {
    pub mob_id: String,
    pub role: String,
    pub member: String,
    pub declared: DeclaredToolCategories,
    pub catalog_exactness: CapabilityCatalogExactness,
}

/// Exactness provenance captured from the actual dispatcher participants.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapabilityCatalogExactness {
    Exact,
    NonExactActualDispatcher,
}

impl MemberCapabilityInvariantContext {
    /// Capture only mob-member builds. Ordinary standalone sessions do not
    /// claim a profile declaration and are outside this invariant.
    #[must_use]
    pub fn from_build_options(build: &SessionBuildOptions) -> Option<Self> {
        let binding = build.mob_member_binding.as_ref()?;
        // Every optional dispatcher here is an ACTUAL participant supplied to
        // the factory. First-party dispatchers composed later by the factory
        // (builtins, comms, memory, schedule/workgraph wrappers, mob,
        // image-generation, web-search, declarative MCP) all report exact
        // catalogs. One non-exact supplied participant makes the final dynamic
        // composite non-exact, and absence must then stay Unverifiable.
        //
        // Meta's own wrappers in these slots (the agent-memory recorder, the
        // SDK callback dispatcher, `ComposedExternalTools`) declare their
        // exactness from what they wrap; a host that supplies a dispatcher on
        // the trait default is the remaining legitimate source of
        // `NonExactCatalog`, and that is a steady state, not a defect.
        let catalog_exactness = if [
            build.external_tools.as_ref(),
            build.schedule_tools.as_ref(),
            build.workgraph_tools.as_ref(),
        ]
        .into_iter()
        .flatten()
        .all(|dispatcher| dispatcher.tool_catalog_capabilities().exact_catalog)
        {
            CapabilityCatalogExactness::Exact
        } else {
            CapabilityCatalogExactness::NonExactActualDispatcher
        };
        Some(Self {
            mob_id: binding.mob_id.clone(),
            role: binding.role.clone(),
            member: binding.member.clone(),
            declared: DeclaredToolCategories::from_build_options(build),
            catalog_exactness,
        })
    }
}

/// Why a live post-materialization comparison has no authoritative answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CapabilityInvariantUnverifiable {
    /// At least one actual dispatcher participating in the final chain says
    /// its catalog is not exact. Absence cannot be interpreted as a gap.
    NonExactCatalog,
    /// The live session service exposes no tool-scope snapshot.
    CatalogUnavailable,
    /// The authoritative catalog read failed.
    CatalogReadFailed(String),
}

impl CapabilityInvariantUnverifiable {
    /// Operator-facing statement of what exactly could not be verified.
    #[must_use]
    pub const fn explanation(&self) -> &'static str {
        match self {
            Self::NonExactCatalog => {
                "a tool dispatcher supplied to this member's build does not declare an \
                 exact catalog, so a tool's absence from the live catalog is not \
                 evidence of a gap"
            }
            Self::CatalogUnavailable => {
                "the live session service exposes no tool-scope snapshot to compare \
                 the declared categories against"
            }
            Self::CatalogReadFailed(_) => "the live tool catalog read failed",
        }
    }
}

/// Authoritative post-materialization observation before policy or category
/// comparison is applied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CapabilityInvariantObservation {
    ExactCatalog {
        declared: DeclaredToolCategories,
        names: BTreeSet<String>,
    },
    Unverifiable {
        declared: DeclaredToolCategories,
        cause: CapabilityInvariantUnverifiable,
    },
}

/// Typed result of one post-materialization invariant evaluation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CapabilityInvariantDecision {
    Match {
        declared: DeclaredToolCategories,
    },
    Gap {
        declared: DeclaredToolCategories,
        missing: Vec<DeclaredToolCategory>,
    },
    Unverifiable {
        declared: DeclaredToolCategories,
        cause: CapabilityInvariantUnverifiable,
    },
}

/// Release-phase enforcement policy. The decision remains typed so moving to
/// park-later is a policy transition, not a log-message convention.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapabilityInvariantPolicy {
    WarnOnly,
    ParkOnMismatch,
}

/// Formal action authorized by a typed decision and rollout policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CapabilityInvariantTransition {
    Continue(CapabilityInvariantDecision),
    WarnAndContinue(CapabilityInvariantDecision),
    ParkRequired(CapabilityInvariantDecision),
}

#[must_use]
pub fn transition_for(
    policy: CapabilityInvariantPolicy,
    decision: CapabilityInvariantDecision,
) -> CapabilityInvariantTransition {
    match (&policy, &decision) {
        (_, CapabilityInvariantDecision::Match { .. }) => {
            CapabilityInvariantTransition::Continue(decision)
        }
        (CapabilityInvariantPolicy::WarnOnly, _) => {
            CapabilityInvariantTransition::WarnAndContinue(decision)
        }
        (CapabilityInvariantPolicy::ParkOnMismatch, _) => {
            CapabilityInvariantTransition::ParkRequired(decision)
        }
    }
}

/// Compare the complete declared category set with one exact live catalog.
#[must_use]
pub fn compare_exact_catalog(
    declared: DeclaredToolCategories,
    names: impl IntoIterator<Item = String>,
) -> CapabilityInvariantDecision {
    decide(CapabilityInvariantObservation::ExactCatalog {
        declared,
        names: names.into_iter().collect(),
    })
}

/// Reduce one typed observation to its comparison decision.
#[must_use]
pub fn decide(observation: CapabilityInvariantObservation) -> CapabilityInvariantDecision {
    match observation {
        CapabilityInvariantObservation::ExactCatalog { declared, names } => {
            let missing = declared
                .iter()
                .filter(|category| !category.is_present_in(&names))
                .collect::<Vec<_>>();
            if missing.is_empty() {
                CapabilityInvariantDecision::Match { declared }
            } else {
                CapabilityInvariantDecision::Gap { declared, missing }
            }
        }
        CapabilityInvariantObservation::Unverifiable { declared, cause } => {
            CapabilityInvariantDecision::Unverifiable { declared, cause }
        }
    }
}

/// Which unverifiable cause a once-per-process notice covers. A failed
/// catalog read is deliberately absent: it is an error every time it happens.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum UnverifiableNoticeKind {
    NonExactCatalog,
    CatalogUnavailable,
}

impl UnverifiableNoticeKind {
    const fn for_cause(cause: &CapabilityInvariantUnverifiable) -> Option<Self> {
        match cause {
            CapabilityInvariantUnverifiable::NonExactCatalog => Some(Self::NonExactCatalog),
            CapabilityInvariantUnverifiable::CatalogUnavailable => Some(Self::CatalogUnavailable),
            CapabilityInvariantUnverifiable::CatalogReadFailed(_) => None,
        }
    }
}

/// Memory of which unverifiable causes this process has already explained.
///
/// Hundreds of members materialize per boot and every one of them shares the
/// same cause, so the explanation is written once per cause; later members
/// keep a per-member DEBUG record for anyone tracing a specific session.
#[derive(Debug, Default)]
pub struct UnverifiableNotice {
    announced: std::sync::Mutex<BTreeSet<UnverifiableNoticeKind>>,
}

impl UnverifiableNotice {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            announced: std::sync::Mutex::new(BTreeSet::new()),
        }
    }

    /// Record `kind` and report whether this is its first sighting.
    fn first_report(&self, kind: UnverifiableNoticeKind) -> bool {
        self.announced
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(kind)
    }
}

static PROCESS_NOTICE: UnverifiableNotice = UnverifiableNotice::new();

/// Log projection of one typed transition. The level policy lives here as a
/// typed decision so a test can pin it without capturing tracing output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CapabilityInvariantReport<'a> {
    /// INFO per member: the exact live catalog carries every declared category.
    Matched {
        declared: &'a DeclaredToolCategories,
    },
    /// WARN per member: an exact live catalog is missing declared categories.
    /// Under `WarnOnly` this is the only outcome that asks for operator action.
    Gap {
        declared: &'a DeclaredToolCategories,
        missing: &'a [DeclaredToolCategory],
    },
    /// INFO once per process per cause: what is unverifiable and that no
    /// action is required unless a member is actually missing tools.
    UnverifiableAnnounced {
        declared: &'a DeclaredToolCategories,
        cause: &'a CapabilityInvariantUnverifiable,
    },
    /// DEBUG per member after the cause has been announced.
    UnverifiableRepeated {
        declared: &'a DeclaredToolCategories,
        cause: &'a CapabilityInvariantUnverifiable,
    },
    /// WARN per member: the live catalog read failed. An error, not a mismatch.
    CatalogReadFailed {
        declared: &'a DeclaredToolCategories,
        error: &'a str,
    },
    /// WARN per member: the rollout policy demands parking for this decision.
    ParkRequired {
        decision: &'a CapabilityInvariantDecision,
    },
}

/// Project a typed transition onto its report, consulting `notice` for the
/// once-per-process unverifiable announcement.
#[must_use]
pub fn report_for<'a>(
    transition: &'a CapabilityInvariantTransition,
    notice: &UnverifiableNotice,
) -> CapabilityInvariantReport<'a> {
    let decision = match transition {
        CapabilityInvariantTransition::ParkRequired(decision) => {
            return CapabilityInvariantReport::ParkRequired { decision };
        }
        CapabilityInvariantTransition::Continue(decision) => {
            debug_assert!(
                matches!(decision, CapabilityInvariantDecision::Match { .. }),
                "only Match may authorize Continue: {decision:?}"
            );
            decision
        }
        CapabilityInvariantTransition::WarnAndContinue(decision) => decision,
    };
    match decision {
        CapabilityInvariantDecision::Match { declared } => {
            CapabilityInvariantReport::Matched { declared }
        }
        CapabilityInvariantDecision::Gap { declared, missing } => CapabilityInvariantReport::Gap {
            declared,
            missing: missing.as_slice(),
        },
        CapabilityInvariantDecision::Unverifiable { declared, cause } => {
            match UnverifiableNoticeKind::for_cause(cause) {
                None => match cause {
                    CapabilityInvariantUnverifiable::CatalogReadFailed(error) => {
                        CapabilityInvariantReport::CatalogReadFailed {
                            declared,
                            error: error.as_str(),
                        }
                    }
                    CapabilityInvariantUnverifiable::NonExactCatalog
                    | CapabilityInvariantUnverifiable::CatalogUnavailable => {
                        CapabilityInvariantReport::UnverifiableRepeated { declared, cause }
                    }
                },
                Some(kind) if notice.first_report(kind) => {
                    CapabilityInvariantReport::UnverifiableAnnounced { declared, cause }
                }
                Some(_) => CapabilityInvariantReport::UnverifiableRepeated { declared, cause },
            }
        }
    }
}

/// Emit the policy projection of a typed transition through the process-wide
/// unverifiable notice.
pub fn emit_transition(
    mob_id: &str,
    role: &str,
    member: &str,
    session_id: &str,
    transition: &CapabilityInvariantTransition,
) {
    emit_report(
        mob_id,
        role,
        member,
        session_id,
        &report_for(transition, &PROCESS_NOTICE),
    );
}

fn emit_report(
    mob_id: &str,
    role: &str,
    member: &str,
    session_id: &str,
    report: &CapabilityInvariantReport<'_>,
) {
    match report {
        CapabilityInvariantReport::Matched { declared } => {
            tracing::info!(
                mob_id,
                role,
                member,
                session_id,
                declared_categories = declared.iter().count(),
                "post-materialization declared-versus-resolved capability invariant matched"
            );
        }
        CapabilityInvariantReport::Gap { declared, missing } => {
            let missing = missing
                .iter()
                .map(|category| category.as_str())
                .collect::<Vec<_>>()
                .join(",");
            tracing::warn!(
                mob_id,
                role,
                member,
                session_id,
                declared = %declared,
                missing = %missing,
                "post-materialization declared-versus-resolved capability invariant found a \
                 verified gap: declared tool categories are missing from the member's live \
                 tool catalog; operator attention required"
            );
        }
        CapabilityInvariantReport::UnverifiableAnnounced { declared, cause } => {
            tracing::info!(
                mob_id,
                role,
                member,
                session_id,
                declared = %declared,
                cause = ?cause,
                "post-materialization declared-versus-resolved capability invariant is \
                 unverifiable for this process: {}. No action is required unless a member \
                 is missing tools it should have; a verified gap is reported separately at \
                 WARN. Further members with this cause are recorded at DEBUG",
                cause.explanation()
            );
        }
        CapabilityInvariantReport::UnverifiableRepeated { declared, cause } => {
            tracing::debug!(
                mob_id,
                role,
                member,
                session_id,
                declared = %declared,
                cause = ?cause,
                "post-materialization declared-versus-resolved capability invariant unverifiable"
            );
        }
        CapabilityInvariantReport::CatalogReadFailed { declared, error } => {
            tracing::warn!(
                mob_id,
                role,
                member,
                session_id,
                declared = %declared,
                error,
                "post-materialization declared-versus-resolved capability invariant could not \
                 read the member's live tool catalog"
            );
        }
        CapabilityInvariantReport::ParkRequired { decision } => {
            tracing::warn!(
                mob_id,
                role,
                member,
                session_id,
                decision = ?decision,
                "post-materialization declared-versus-resolved capability invariant requires \
                 operator attention: the rollout policy parks this member"
            );
        }
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod tests {
    use std::sync::Arc;

    use async_trait::async_trait;

    use super::*;

    struct CatalogDispatcher {
        exact: bool,
    }

    #[async_trait]
    impl meerkat_core::AgentToolDispatcher for CatalogDispatcher {
        fn tools(&self) -> Arc<[Arc<meerkat_core::types::ToolDef>]> {
            Vec::new().into()
        }

        fn tool_catalog_capabilities(&self) -> meerkat_core::ToolCatalogCapabilities {
            meerkat_core::ToolCatalogCapabilities {
                exact_catalog: self.exact,
                may_require_catalog_control_plane: false,
            }
        }

        async fn dispatch(
            &self,
            call: meerkat_core::types::ToolCallView<'_>,
        ) -> Result<meerkat_core::ToolDispatchOutcome, meerkat_core::ToolError> {
            Err(meerkat_core::ToolError::not_found(call.name))
        }
    }

    fn member_build() -> SessionBuildOptions {
        SessionBuildOptions {
            mob_member_binding: Some(meerkat_core::MobMemberBinding {
                mob_id: "test-mob".to_string(),
                role: "worker".to_string(),
                member: "worker-1".to_string(),
            }),
            ..build_with_all_categories()
        }
    }

    fn build_with_all_categories() -> SessionBuildOptions {
        SessionBuildOptions {
            override_builtins: ToolCategoryOverride::Enable,
            override_shell: ToolCategoryOverride::Enable,
            override_comms: ToolCategoryOverride::Enable,
            override_memory: ToolCategoryOverride::Enable,
            override_schedule: ToolCategoryOverride::Enable,
            override_workgraph: ToolCategoryOverride::Enable,
            override_mob: ToolCategoryOverride::Enable,
            override_image_generation: ToolCategoryOverride::Enable,
            override_web_search: ToolCategoryOverride::Enable,
            ..Default::default()
        }
    }

    fn full_catalog() -> Vec<String> {
        [
            "task_create",
            "shell",
            "send_message",
            "memory_search",
            "meerkat_schedule_list",
            "workgraph_get",
            "spawn_member",
            "generate_image",
            "web_search",
        ]
        .into_iter()
        .map(str::to_string)
        .collect()
    }

    #[test]
    fn captures_the_complete_nine_category_declaration_set() {
        let declared = DeclaredToolCategories::from_build_options(&build_with_all_categories());
        assert_eq!(
            declared.iter().collect::<Vec<_>>(),
            DeclaredToolCategory::ALL
        );
    }

    #[test]
    fn mob_presence_vocabulary_is_pinned() {
        // This list restates meerkat's mob tool surface. Meta cannot see a
        // rename upstream: a dropped name does not fail to compile, it turns
        // into a false `Gap` decision that parks a healthy member. Changing
        // this set must be a decision someone made, not a diff someone made.
        assert_eq!(
            MOB_UNPREFIXED_TOOL_NAMES,
            &[
                "spawn_member",
                "spawn_many_members",
                "retire_member",
                "wire_members",
                "unwire_members",
                "list_members",
                "member_status",
                "force_cancel_member",
                "fork_off",
                "delegate",
            ],
            "the mob presence vocabulary changed. Confirm against meerkat's \
             mob tool surface that the new set is what upstream actually \
             exposes, then update this assertion deliberately."
        );
    }

    #[test]
    fn delegate_alone_proves_the_mob_surface_is_present() {
        // `delegate` is a first-party mob tool. Before 0.8.28 the classifier
        // missed it: a catalog carrying only `delegate` read as a Gap against
        // a declared Mob category. Latent rather than live, because meerkat
        // enables the mob tools as a group and `mob_`-prefixed siblings have
        // always co-occurred - but the classifier should not depend on that.
        let names = BTreeSet::from(["delegate".to_string()]);
        assert!(DeclaredToolCategory::Mob.is_present_in(&names));
    }

    #[test]
    fn fork_off_is_a_mob_capability_marker() {
        let names = BTreeSet::from(["fork_off".to_string()]);
        assert!(DeclaredToolCategory::Mob.is_present_in(&names));
    }

    #[test]
    fn exact_catalog_is_an_actual_dispatcher_witness_not_config_inference() {
        let exact = MemberCapabilityInvariantContext::from_build_options(&member_build())
            .expect("member context");
        assert_eq!(exact.catalog_exactness, CapabilityCatalogExactness::Exact);

        for slot in ["external", "schedule", "workgraph"] {
            let mut build = member_build();
            let dispatcher = Arc::new(CatalogDispatcher { exact: false });
            match slot {
                "external" => build.external_tools = Some(dispatcher),
                "schedule" => build.schedule_tools = Some(dispatcher),
                "workgraph" => build.workgraph_tools = Some(dispatcher),
                _ => unreachable!(),
            }
            let context = MemberCapabilityInvariantContext::from_build_options(&build)
                .expect("member context");
            assert!(
                context.catalog_exactness == CapabilityCatalogExactness::NonExactActualDispatcher,
                "a non-exact {slot} dispatcher must make absence unverifiable"
            );
        }
    }

    #[test]
    fn every_category_is_mutation_sensitive() {
        let declared = DeclaredToolCategories::from_build_options(&build_with_all_categories());
        let full = full_catalog();
        assert!(matches!(
            compare_exact_catalog(declared.clone(), full.clone()),
            CapabilityInvariantDecision::Match { .. }
        ));

        for (index, category) in DeclaredToolCategory::ALL.into_iter().enumerate() {
            let mut mutated = full.clone();
            mutated.remove(index);
            assert_eq!(
                compare_exact_catalog(declared.clone(), mutated),
                CapabilityInvariantDecision::Gap {
                    declared: declared.clone(),
                    missing: vec![category],
                },
                "removing the canonical {category} marker must create exactly that gap"
            );
        }
    }

    #[test]
    fn warn_first_and_park_later_are_typed_policy_transitions() {
        let declared = DeclaredToolCategories::from_build_options(&build_with_all_categories());
        let gap = CapabilityInvariantDecision::Gap {
            declared,
            missing: vec![DeclaredToolCategory::Schedule],
        };
        assert!(matches!(
            transition_for(CapabilityInvariantPolicy::WarnOnly, gap.clone()),
            CapabilityInvariantTransition::WarnAndContinue(decision) if decision == gap
        ));
        assert!(matches!(
            transition_for(CapabilityInvariantPolicy::ParkOnMismatch, gap.clone()),
            CapabilityInvariantTransition::ParkRequired(decision) if decision == gap
        ));

        let unverifiable = decide(CapabilityInvariantObservation::Unverifiable {
            declared: DeclaredToolCategories::default(),
            cause: CapabilityInvariantUnverifiable::NonExactCatalog,
        });
        assert!(matches!(
            transition_for(CapabilityInvariantPolicy::WarnOnly, unverifiable.clone()),
            CapabilityInvariantTransition::WarnAndContinue(decision) if decision == unverifiable
        ));
        assert!(matches!(
            transition_for(
                CapabilityInvariantPolicy::ParkOnMismatch,
                unverifiable.clone()
            ),
            CapabilityInvariantTransition::ParkRequired(decision) if decision == unverifiable
        ));
    }

    #[test]
    fn inherit_and_disable_are_not_declarations() {
        let mut build = build_with_all_categories();
        build.override_shell = ToolCategoryOverride::Disable;
        build.override_memory = ToolCategoryOverride::Inherit;
        let declared = DeclaredToolCategories::from_build_options(&build);
        assert!(
            !declared
                .iter()
                .any(|category| category == DeclaredToolCategory::Shell)
        );
        assert!(
            !declared
                .iter()
                .any(|category| category == DeclaredToolCategory::Memory)
        );
        assert_eq!(declared.iter().count(), 7);
    }

    fn unverifiable_transition(
        cause: CapabilityInvariantUnverifiable,
    ) -> CapabilityInvariantTransition {
        transition_for(
            CapabilityInvariantPolicy::WarnOnly,
            decide(CapabilityInvariantObservation::Unverifiable {
                declared: DeclaredToolCategories::from_build_options(&build_with_all_categories()),
                cause,
            }),
        )
    }

    #[test]
    fn a_match_reports_matched_per_member() {
        let declared = DeclaredToolCategories::from_build_options(&build_with_all_categories());
        let transition = transition_for(
            CapabilityInvariantPolicy::WarnOnly,
            compare_exact_catalog(declared.clone(), full_catalog()),
        );
        let notice = UnverifiableNotice::new();
        assert_eq!(
            report_for(&transition, &notice),
            CapabilityInvariantReport::Matched {
                declared: &declared
            }
        );
    }

    #[test]
    fn a_verified_gap_is_the_only_warn_only_outcome_asking_for_operator_action() {
        let declared = DeclaredToolCategories::from_build_options(&build_with_all_categories());
        let mut catalog = full_catalog();
        catalog.retain(|name| name != "workgraph_get");
        let transition = transition_for(
            CapabilityInvariantPolicy::WarnOnly,
            compare_exact_catalog(declared.clone(), catalog),
        );
        let notice = UnverifiableNotice::new();
        assert_eq!(
            report_for(&transition, &notice),
            CapabilityInvariantReport::Gap {
                declared: &declared,
                missing: &[DeclaredToolCategory::WorkGraph],
            }
        );
    }

    #[test]
    fn non_exact_catalog_is_announced_once_per_process_then_recorded_per_member() {
        let notice = UnverifiableNotice::new();
        let first = unverifiable_transition(CapabilityInvariantUnverifiable::NonExactCatalog);
        let second = unverifiable_transition(CapabilityInvariantUnverifiable::NonExactCatalog);
        let third = unverifiable_transition(CapabilityInvariantUnverifiable::NonExactCatalog);

        assert!(matches!(
            report_for(&first, &notice),
            CapabilityInvariantReport::UnverifiableAnnounced {
                cause: CapabilityInvariantUnverifiable::NonExactCatalog,
                ..
            }
        ));
        for later in [&second, &third] {
            assert!(matches!(
                report_for(later, &notice),
                CapabilityInvariantReport::UnverifiableRepeated {
                    cause: CapabilityInvariantUnverifiable::NonExactCatalog,
                    ..
                }
            ));
        }
    }

    #[test]
    fn each_unverifiable_cause_gets_its_own_announcement() {
        let notice = UnverifiableNotice::new();
        let non_exact = unverifiable_transition(CapabilityInvariantUnverifiable::NonExactCatalog);
        let unavailable =
            unverifiable_transition(CapabilityInvariantUnverifiable::CatalogUnavailable);
        assert!(matches!(
            report_for(&non_exact, &notice),
            CapabilityInvariantReport::UnverifiableAnnounced { .. }
        ));
        assert!(matches!(
            report_for(&unavailable, &notice),
            CapabilityInvariantReport::UnverifiableAnnounced {
                cause: CapabilityInvariantUnverifiable::CatalogUnavailable,
                ..
            }
        ));
        assert!(matches!(
            report_for(&unavailable, &notice),
            CapabilityInvariantReport::UnverifiableRepeated { .. }
        ));
    }

    #[test]
    fn a_failed_catalog_read_is_reported_as_an_error_every_time() {
        let notice = UnverifiableNotice::new();
        let failed = unverifiable_transition(CapabilityInvariantUnverifiable::CatalogReadFailed(
            "store offline".to_string(),
        ));
        for _ in 0..2 {
            assert!(matches!(
                report_for(&failed, &notice),
                CapabilityInvariantReport::CatalogReadFailed {
                    error: "store offline",
                    ..
                }
            ));
        }
    }

    #[test]
    fn park_required_always_reports_for_operator_attention() {
        let notice = UnverifiableNotice::new();
        let decision = decide(CapabilityInvariantObservation::Unverifiable {
            declared: DeclaredToolCategories::default(),
            cause: CapabilityInvariantUnverifiable::NonExactCatalog,
        });
        let transition =
            transition_for(CapabilityInvariantPolicy::ParkOnMismatch, decision.clone());
        assert_eq!(
            report_for(&transition, &notice),
            CapabilityInvariantReport::ParkRequired {
                decision: &decision
            }
        );
    }

    #[test]
    fn unverifiable_causes_explain_themselves() {
        assert!(
            CapabilityInvariantUnverifiable::NonExactCatalog
                .explanation()
                .contains("does not declare an exact catalog")
        );
        assert!(
            CapabilityInvariantUnverifiable::CatalogUnavailable
                .explanation()
                .contains("no tool-scope snapshot")
        );
    }

    #[test]
    fn declared_categories_display_as_a_stable_list() {
        assert_eq!(DeclaredToolCategories::default().to_string(), "none");
        let build = SessionBuildOptions {
            override_mob: ToolCategoryOverride::Enable,
            override_builtins: ToolCategoryOverride::Enable,
            ..Default::default()
        };
        assert_eq!(
            DeclaredToolCategories::from_build_options(&build).to_string(),
            "builtins,mob"
        );
    }
}
