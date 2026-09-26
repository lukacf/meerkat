//! What a fork-derived member inherits from its source member's build.
//!
//! A durable fork copies its source's transcript, but not the per-build inputs
//! the source's host resolved its tools and instructions from: the source's
//! application context, its application labels, and its retained per-spawn
//! tool overlay. A child built without them is a different agent. HomeCore saw
//! a calendar fork built as a generic domain member: 92 of calendar's 150
//! tools, no calendar, display or picture-schedule tools, no `memory` tool,
//! and a tools block that differed from the forker's, so the child could not
//! reuse the forker's cached prefix either.
//!
//! [`ForkBuildInheritance`] carries those inputs, with the typed
//! [`meerkat_core::ForkBuildSource`] a host resolves the child by, from the
//! source member to the child's seating build. It applies to every
//! fork-derived member: `fork_off` children and other
//! [`MobHandle::fork_member`](super::MobHandle::fork_member) forks, and
//! temporary-council participants forked from a convener's member. Delegate
//! helpers, live-delegation workers and ordinary spawns have their own spec
//! and never carry it.
//!
//! The child keeps its own roster, comms and runtime identity. Only the build
//! inputs are the source's.

use std::collections::BTreeMap;
use std::sync::Arc;

use meerkat_core::types::SessionId;
use meerkat_core::{AgentToolDispatcher, ForkBuildSource};

use crate::ids::AgentIdentity;

/// Build inputs a fork-derived member inherits from its source member.
///
/// Minted only by the source member's own mob, from its roster entry, its
/// retained per-spawn overlay and the source session's durable build state
/// (see [`MobHandle::fork_build_inheritance`](super::MobHandle::fork_build_inheritance)).
/// A caller therefore cannot name a source it did not fork from. The value is
/// opaque: the runtime applies it to the child's spawn and it is never edited.
///
/// Applying it to a child spawn sets the child's typed fork source and fills
/// the child's application context, labels and per-spawn tool overlay from the
/// source. A value the child's own spawn request states explicitly is kept: the
/// source supplies the default, the request is the override. For labels that
/// holds per key.
#[derive(Clone)]
pub struct ForkBuildInheritance {
    source: ForkBuildSource,
    app_context: Option<serde_json::Value>,
    labels: BTreeMap<String, String>,
    external_tools: Option<Arc<dyn AgentToolDispatcher>>,
}

impl ForkBuildInheritance {
    pub(crate) fn new(
        source: ForkBuildSource,
        app_context: Option<serde_json::Value>,
        labels: BTreeMap<String, String>,
        external_tools: Option<Arc<dyn AgentToolDispatcher>>,
    ) -> Self {
        Self {
            source,
            app_context,
            labels,
            external_tools,
        }
    }

    /// The typed source the child's build will carry.
    pub fn source(&self) -> &ForkBuildSource {
        &self.source
    }

    /// The application context of the source's current build, verbatim.
    pub fn app_context(&self) -> Option<&serde_json::Value> {
        self.app_context.as_ref()
    }

    /// The source's application labels, verbatim. Standard mob labels
    /// (`mob_id`, `role`, `agent_identity`, ...) are not among them: the
    /// child's build stamps its own.
    pub fn labels(&self) -> &BTreeMap<String, String> {
        &self.labels
    }

    /// Whether the source carries a retained per-spawn tool overlay.
    pub fn has_external_tools(&self) -> bool {
        self.external_tools.is_some()
    }

    /// Whether this inheritance was minted for the fork of exactly
    /// `source_identity`'s session `source_session_id`.
    pub(crate) fn names_fork_of(
        &self,
        source_identity: &AgentIdentity,
        source_session_id: &SessionId,
    ) -> bool {
        self.source.source_member.member == source_identity.as_str()
            && &self.source.source_session_id == source_session_id
    }

    /// Apply to the child's spawn request (see the type docs).
    pub(crate) fn apply_to(self, spec: &mut super::handle::SpawnMemberSpec) {
        let Self {
            source,
            app_context,
            labels,
            external_tools,
        } = self;
        spec.fork_source = Some(source);
        if spec.context.is_none() {
            spec.context = app_context;
        }
        if !labels.is_empty() {
            let mut merged = labels;
            if let Some(requested) = spec.labels.take() {
                merged.extend(requested);
            }
            spec.labels = Some(merged);
        }
        if spec.external_tools.is_none() {
            spec.external_tools = external_tools;
        }
    }
}

impl std::fmt::Debug for ForkBuildInheritance {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ForkBuildInheritance")
            .field("source", &self.source)
            .field("app_context", &self.app_context.is_some())
            .field("labels", &self.labels)
            .field("external_tools", &self.external_tools.is_some())
            .finish()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::ids::ProfileName;
    use crate::runtime::SpawnMemberSpec;

    struct NoTools;

    #[async_trait::async_trait]
    impl AgentToolDispatcher for NoTools {
        fn tools(&self) -> Arc<[Arc<meerkat_core::ToolDef>]> {
            Vec::new().into()
        }

        async fn dispatch(
            &self,
            call: meerkat_core::ToolCallView<'_>,
        ) -> Result<meerkat_core::ToolDispatchOutcome, meerkat_core::ToolError> {
            Err(meerkat_core::ToolError::not_found(call.name))
        }
    }

    fn source() -> ForkBuildSource {
        ForkBuildSource::new(
            meerkat_core::MobMemberBinding {
                mob_id: "home".to_string(),
                role: "domain".to_string(),
                member: "domain-calendar".to_string(),
            },
            SessionId::new(),
        )
    }

    fn inheritance(tools: Option<Arc<dyn AgentToolDispatcher>>) -> ForkBuildInheritance {
        ForkBuildInheritance::new(
            source(),
            Some(serde_json::json!({"domain": "calendar"})),
            BTreeMap::from([
                ("domain".to_string(), "calendar".to_string()),
                ("tier".to_string(), "gold".to_string()),
            ]),
            tools,
        )
    }

    #[test]
    fn a_bare_child_spec_takes_every_source_input() {
        let tools: Arc<dyn AgentToolDispatcher> = Arc::new(NoTools);
        let inheritance = inheritance(Some(Arc::clone(&tools)));
        let expected_source = inheritance.source().clone();
        let mut spec = SpawnMemberSpec::new(ProfileName::from("domain"), "child");

        inheritance.apply_to(&mut spec);

        assert_eq!(spec.fork_source, Some(expected_source));
        assert_eq!(
            spec.context,
            Some(serde_json::json!({"domain": "calendar"}))
        );
        assert_eq!(
            spec.labels,
            Some(BTreeMap::from([
                ("domain".to_string(), "calendar".to_string()),
                ("tier".to_string(), "gold".to_string()),
            ]))
        );
        assert!(
            spec.external_tools
                .as_ref()
                .is_some_and(|applied| Arc::ptr_eq(applied, &tools)),
            "the child carries the source's exact overlay dispatcher"
        );
    }

    #[test]
    fn explicit_child_inputs_override_the_source_defaults() {
        let source_tools: Arc<dyn AgentToolDispatcher> = Arc::new(NoTools);
        let child_tools: Arc<dyn AgentToolDispatcher> = Arc::new(NoTools);
        let mut spec = SpawnMemberSpec::new(ProfileName::from("domain"), "child");
        spec.context = Some(serde_json::json!({"domain": "override"}));
        spec.labels = Some(BTreeMap::from([("tier".to_string(), "silver".to_string())]));
        spec.external_tools = Some(Arc::clone(&child_tools));

        inheritance(Some(source_tools)).apply_to(&mut spec);

        assert_eq!(
            spec.context,
            Some(serde_json::json!({"domain": "override"}))
        );
        assert_eq!(
            spec.labels,
            Some(BTreeMap::from([
                ("domain".to_string(), "calendar".to_string()),
                ("tier".to_string(), "silver".to_string()),
            ])),
            "the source supplies unlabelled keys; the request wins per key"
        );
        assert!(
            spec.external_tools
                .as_ref()
                .is_some_and(|applied| Arc::ptr_eq(applied, &child_tools))
        );
        assert!(spec.fork_source.is_some());
    }

    #[test]
    fn names_fork_of_requires_the_exact_source_member_and_session() {
        let inheritance = inheritance(None);
        let session = inheritance.source().source_session_id.clone();
        assert!(inheritance.names_fork_of(&AgentIdentity::from("domain-calendar"), &session));
        assert!(!inheritance.names_fork_of(&AgentIdentity::from("domain-gmail"), &session));
        assert!(
            !inheritance.names_fork_of(&AgentIdentity::from("domain-calendar"), &SessionId::new())
        );
    }
}
