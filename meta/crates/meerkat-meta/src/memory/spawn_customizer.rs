//! Classic-mob agent memory (docs/design/agent-memory-architecture.md §8.2,
//! §9.1) without the identity-first orchestration layer.
//!
//! Memory is keyed by [`AgentIdentity`], which every mob member already has —
//! it does not require the continuity/roster machinery. This module carries
//! the BASIC memory surface onto the classic mob path through meerkat-mob's
//! pre-build seam ([`meerkat_mob::SpawnMemberCustomizer`], applied to every
//! member spawn including resume restores): the Recorder `memory` tool and
//! the echo-safe build-time injection block. The ADVANCED lifecycle features
//! (distill-on-respawn/reset/retire, exit interviews, per-turn ambient
//! injection) stay bound to `IdentityRuntime`.
//!
//! Per-turn injection on the classic path is a deliberate no-op: the P0.1
//! echo-safety default is `AgentMemoryPerTurnInjection::Off`, and the classic
//! send path has no injection hook yet. A classic-mob per-turn hook is a
//! scoped follow-up (see the design doc §9.1); until then `budgeted` only
//! takes effect on identity-first members.

use std::collections::BTreeSet;
use std::sync::Arc;

use meerkat_mob::{
    MobError, SpawnCustomizationContext, SpawnMemberCustomizer, SpawnMemberSpec, ids::MobId,
};

use crate::identity_first::AgentIdentity;
use crate::identity_first::agent_memory::{
    AgentMemoryConfig, AgentMemoryError, AgentMemoryPerTurnInjection, AgentMemoryProvider,
    MemberMemoryKept, MemoryRecorder, RECORDER_PROTOCOL_INSTRUCTIONS, RecorderToolDispatcher,
    compact_whitespace, insert_terms,
};
use crate::memory::coordinator::RecallCoordinator;

/// Per-spawn memory customizer for classic (roster-less) mobs. One instance
/// serves the whole mob runtime; recorder dispatchers are re-created per
/// spawn, so the tool surface stays restore-safe exactly like the
/// identity-first `customize_build` path.
pub struct MemorySpawnCustomizer {
    coordinator: RecallCoordinator,
    /// Who each member of the mob is private to, where the composition
    /// installed this customizer on a mob that keeps that record: it says
    /// who a spawn will be private to before the spawn's labels do.
    private_owners: Option<crate::private_owners::PrivateOwners>,
}

impl MemorySpawnCustomizer {
    pub fn new(provider: Arc<dyn AgentMemoryProvider>, config: AgentMemoryConfig) -> Self {
        if config.per_turn_injection == AgentMemoryPerTurnInjection::Budgeted {
            // `Budgeted` is now the platform default (ask 1 made ambient
            // injection echo-safe), so this is no longer a user misconfig —
            // it is an as-designed limitation of the classic path, which
            // still injects at build time only (§9.1). Debug, not warn, to
            // avoid spamming every roster-less mob that just takes defaults.
            tracing::debug!(
                "agent_memory.per_turn_injection = budgeted: ambient per-turn injection is \
                 identity-first-only for now; the classic mob path injects at build time only \
                 (the recorder tool and build-time injection are unaffected)"
            );
        }
        Self {
            coordinator: RecallCoordinator::new(provider, config),
            private_owners: None,
        }
    }

    /// This customizer, reading who a spawn will be private to from
    /// `owners`, the owner record of the mob it is installed on. The mob's
    /// owner propagation runs after this customizer, so a member that a
    /// private member asks for carries no owner label yet when it is asked.
    pub(crate) fn with_private_owners(
        mut self,
        owners: crate::private_owners::PrivateOwners,
    ) -> Self {
        self.private_owners = Some(owners);
        self
    }

    /// The person the member `spec` spawns will be private to, or `None`
    /// for a member private to nobody: as the mob's owner record decides
    /// it, or, without one, as the label the spec carries says. `Err` for a
    /// spawn the owner propagation will refuse.
    fn private_owner(
        &self,
        mob_id: &MobId,
        spawner_identity: Option<&meerkat_mob::ids::AgentIdentity>,
        spec: &SpawnMemberSpec,
    ) -> Result<Option<String>, MobError> {
        match self.private_owners.as_ref() {
            Some(owners) => owners.owner_a_spawn_takes(mob_id.as_str(), spawner_identity, spec),
            None => Ok(spec
                .labels
                .as_ref()
                .and_then(crate::access::private_owner)
                .map(str::to_string)),
        }
    }

    /// The customizer body, factored off the trait so tests can drive it:
    /// `SpawnCustomizationContext` is `#[non_exhaustive]` and only meerkat-mob
    /// constructs it.
    fn apply(
        &self,
        mob_id: &MobId,
        spawner_identity: Option<&meerkat_mob::ids::AgentIdentity>,
        spec: &mut SpawnMemberSpec,
    ) -> Result<(), MobError> {
        // Memory scope keys pin to the LOGICAL identity (task #53) - the
        // identity space the console, panel filters, persisted records, and
        // the SDK agent_memory surface speak - never the comms-safe roster
        // encoding meerkat-mob spawns with (`agent:mem` arrives here as
        // `mk--agent_cmem`) and never a generated runtime alias
        // (`rt:{identity}:{generation}` strips to the durable identity, so
        // respawn generations share one scope). A member alias that fails
        // validation gets no memory surface - loudly, never silently -
        // instead of blocking the spawn.
        let alias = crate::member_comms_id::logical_memory_identity(spec.identity.as_str());
        let identity = match AgentIdentity::parse(&alias) {
            Ok(identity) => identity,
            Err(err) => {
                tracing::warn!(
                    identity = %spec.identity,
                    error = %err,
                    "agent memory skipped for member: identity fails memory-scope validation"
                );
                return Ok(());
            }
        };

        // A member private to a person keeps its memory under its owner and
        // its name: the records under its bare name are those of every
        // member of that name, whoever it is private to, so it is given
        // none of them here. It recalls its own with its `memory` tool. A
        // spawn the owner propagation will refuse gets no memory surface.
        let private_owner = match self.private_owner(mob_id, spawner_identity, spec) {
            Ok(owner) => owner,
            Err(_) => return Ok(()),
        };
        if private_owner.is_none() {
            let injection = block_on_build_injection(
                &self.coordinator,
                &identity,
                spawn_query_text(mob_id, &identity, spec),
                spawn_query_terms(mob_id, &identity, spawner_identity, spec),
            )
            .map_err(|err| {
                MobError::Internal(format!(
                    "agent memory build injection failed for '{}': {err}",
                    identity.as_str()
                ))
            })?;
            if let Some(injection) = injection
                && !injection.is_empty()
            {
                spec.additional_instructions
                    .get_or_insert_with(Vec::new)
                    .push(injection);
            }
        }

        // §8.2 Recorder: same capability gate as the identity-first path.
        // The dispatcher composes over any per-spawn external-tool overlay
        // already on the spec; meerkat-mob then composes the result with
        // profile bundles and mob-wide defaults (profile tools win name
        // collisions). A private member's recorder runs inside no mob and
        // keeps its records under its owner, as the build seam binds it
        // (`crate::memory::side_chat_memory`).
        let config = self.coordinator.config();
        let provider = self.coordinator.provider();
        if config.recorder_tool && provider.supports_authored_writes() {
            let recorder = MemoryRecorder::new(
                provider,
                config,
                identity,
                private_owner.is_none().then(|| mob_id.to_string()),
            );
            let recorder = match private_owner {
                Some(owner) => recorder.kept(MemberMemoryKept::UnderItsOwner(owner)),
                None => recorder,
            };
            let inner = spec.external_tools.take();
            spec.external_tools = Some(Arc::new(RecorderToolDispatcher::new(inner, recorder)));
            spec.additional_instructions
                .get_or_insert_with(Vec::new)
                .push(RECORDER_PROTOCOL_INSTRUCTIONS.to_string());
        }
        Ok(())
    }
}

impl SpawnMemberCustomizer for MemorySpawnCustomizer {
    fn customize_spawn(
        &self,
        ctx: &SpawnCustomizationContext,
        spec: &mut SpawnMemberSpec,
    ) -> Result<(), MobError> {
        self.apply(&ctx.mob_id, ctx.spawner_identity.as_ref(), spec)
    }
}

/// Run the async build-injection assembly from meerkat-mob's synchronous
/// `customize_spawn` seam. A dedicated thread with its own current-thread
/// runtime keeps this correct on any caller runtime flavor (no
/// `block_in_place` panic on current-thread runtimes, and the coordinator's
/// internal `tokio::time::timeout`s get a live timer driver). Bounded by the
/// coordinator's own recall budget (`recall_timeout_ms`; the 2× build-time
/// extension retired with the §8.3 selector stage), so a spawn never hangs
/// on memory.
fn block_on_build_injection(
    coordinator: &RecallCoordinator,
    identity: &AgentIdentity,
    query_text: Option<String>,
    query_terms: Vec<String>,
) -> Result<Option<String>, AgentMemoryError> {
    std::thread::scope(|scope| {
        scope
            .spawn(|| {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|err| {
                        AgentMemoryError::Io(format!(
                            "memory recall runtime failed to start: {err}"
                        ))
                    })?;
                runtime.block_on(coordinator.assemble_build_injection(
                    identity,
                    query_text,
                    query_terms,
                ))
            })
            .join()
            .unwrap_or_else(|_| {
                Err(AgentMemoryError::Io(
                    "memory build-injection assembly panicked".to_string(),
                ))
            })
    })
}

/// Classic-path counterpart of the identity-first `build_query_text`: the
/// spawn spec has no continuity context (no active peers or managed edges),
/// so the query composes identity + profile + labels + mob.
fn spawn_query_text(
    mob_id: &MobId,
    identity: &AgentIdentity,
    spec: &SpawnMemberSpec,
) -> Option<String> {
    let mut parts = vec![
        format!("identity {}", identity.as_str()),
        format!("profile {}", spec.role_name),
        format!("mob {mob_id}"),
    ];
    if let Some(labels) = spec.labels.as_ref() {
        for (key, value) in labels {
            parts.push(format!("label {key} {value}"));
        }
    }
    let text = compact_whitespace(&parts.join(" "));
    (!text.is_empty()).then_some(text)
}

fn spawn_query_terms(
    mob_id: &MobId,
    identity: &AgentIdentity,
    spawner_identity: Option<&meerkat_mob::ids::AgentIdentity>,
    spec: &SpawnMemberSpec,
) -> Vec<String> {
    let mut terms = BTreeSet::new();
    insert_terms(&mut terms, identity.as_str());
    insert_terms(&mut terms, spec.role_name.as_str());
    insert_terms(&mut terms, mob_id.as_str());
    if let Some(spawner) = spawner_identity {
        insert_terms(&mut terms, spawner.as_str());
    }
    if let Some(labels) = spec.labels.as_ref() {
        for (key, value) in labels {
            insert_terms(&mut terms, key);
            insert_terms(&mut terms, value);
        }
    }
    terms.into_iter().collect()
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::memory::records::MemoryAuthor;
    use crate::memory::sqlite_store::SqliteAgentMemoryStore;
    use meerkat_core::agent::AgentToolDispatcher;
    use meerkat_mob::ProfileName;
    use meerkat_mob::ids::AgentIdentity as MobIdentity;

    struct RecallOnlyProvider;

    #[async_trait::async_trait]
    impl AgentMemoryProvider for RecallOnlyProvider {
        async fn recall(
            &self,
            _request: crate::identity_first::AgentMemoryRecallRequest,
        ) -> Result<Vec<crate::identity_first::AgentMemoryRecord>, AgentMemoryError> {
            Ok(Vec::new())
        }
    }

    fn sqlite_store(dir: &std::path::Path) -> Arc<SqliteAgentMemoryStore> {
        Arc::new(SqliteAgentMemoryStore::open(dir).expect("open sqlite store"))
    }

    fn spec_for(identity: &str) -> SpawnMemberSpec {
        SpawnMemberSpec::new(ProfileName::from("worker"), MobIdentity::from(identity))
    }

    async fn seed_record(store: &SqliteAgentMemoryStore, identity: &str, title: &str, body: &str) {
        let scope = crate::memory::records::MemoryScope::Identity {
            realm: "default".to_string(),
            identity: identity.to_string(),
        };
        store
            .remember_authored(
                &scope,
                crate::memory::records::NewMemoryRecord {
                    kind: crate::memory::records::MemoryKind::Fact,
                    title: title.to_string(),
                    description: title.to_string(),
                    body: body.to_string(),
                    tags: vec![],
                    evidence: vec![],
                    verification: None,
                },
                MemoryAuthor::Operator { principal: None },
            )
            .await
            .expect("seed memory record");
    }

    #[test]
    fn registers_recorder_tool_and_protocol_on_spawn_spec() {
        let dir = tempfile::tempdir().expect("temp dir");
        let store = sqlite_store(dir.path());
        let customizer = MemorySpawnCustomizer::new(store, AgentMemoryConfig::default());

        let mut spec = spec_for("agent:mem");
        customizer
            .apply(&MobId::from("test-mob"), None, &mut spec)
            .expect("apply succeeds");

        let tools = spec.external_tools.as_ref().expect("recorder registered");
        assert!(
            tools
                .tools()
                .iter()
                .any(|tool| tool.name.as_ref() == crate::identity_first::MEMORY_TOOL_NAME),
            "memory tool must be registered on the spawn spec"
        );
        let instructions = spec.additional_instructions.as_ref().expect("instructions");
        assert!(
            instructions
                .iter()
                .any(|section| section.contains("Memory recorder protocol")),
            "recorder protocol instructions must be injected: {instructions:#?}"
        );
    }

    #[tokio::test]
    async fn injects_build_time_memory_block_for_seeded_identity() {
        let dir = tempfile::tempdir().expect("temp dir");
        let store = sqlite_store(dir.path());
        seed_record(
            &store,
            "agent:mem",
            "Deploy window",
            "Deploys are frozen on Fridays.",
        )
        .await;
        let customizer = MemorySpawnCustomizer::new(
            store.clone(),
            AgentMemoryConfig {
                selection: crate::identity_first::AgentMemorySelection::Always,
                ..AgentMemoryConfig::default()
            },
        );

        // Spawn specs arrive with the comms-safe roster encoding; memory
        // must key on the decoded public alias (`agent:mem`).
        let mut spec = spec_for(crate::member_comms_id::mob_member_id_str("agent:mem").as_ref());
        assert_eq!(spec.identity.as_str(), "mk--agent_cmem");
        customizer
            .apply(&MobId::from("test-mob"), None, &mut spec)
            .expect("apply succeeds");

        let instructions = spec.additional_instructions.as_ref().expect("instructions");
        assert!(
            instructions
                .iter()
                .any(|section| section.contains("Deploys are frozen on Fridays.")),
            "build-time injection must carry the seeded record body: {instructions:#?}"
        );
    }

    #[test]
    fn recorder_composes_over_existing_per_spawn_overlay() {
        struct EchoDispatcher;
        #[async_trait::async_trait]
        impl AgentToolDispatcher for EchoDispatcher {
            fn tools(&self) -> Arc<[Arc<meerkat_core::ToolDef>]> {
                vec![Arc::new(meerkat_core::ToolDef {
                    audience: Default::default(),
                    name: "echo".into(),
                    description: "echo".to_string(),
                    input_schema: serde_json::json!({"type": "object"}),
                    provenance: None,
                })]
                .into()
            }
            async fn dispatch(
                &self,
                call: meerkat_core::types::ToolCallView<'_>,
            ) -> Result<meerkat_core::ops::ToolDispatchOutcome, meerkat_core::error::ToolError>
            {
                Ok(meerkat_core::ToolResult {
                    host_metadata: Default::default(),
                    settlement_failures: Default::default(),
                    tool_use_id: call.id.to_string(),
                    content: vec![],
                    is_error: false,
                }
                .into())
            }
        }

        let dir = tempfile::tempdir().expect("temp dir");
        let store = sqlite_store(dir.path());
        let customizer = MemorySpawnCustomizer::new(store, AgentMemoryConfig::default());

        let mut spec = spec_for("agent:mem");
        spec.external_tools = Some(Arc::new(EchoDispatcher));
        customizer
            .apply(&MobId::from("test-mob"), None, &mut spec)
            .expect("apply succeeds");

        let tools = spec.external_tools.as_ref().expect("dispatcher present");
        let names: Vec<String> = tools
            .tools()
            .iter()
            .map(|tool| tool.name.to_string())
            .collect();
        assert!(names.contains(&"echo".to_string()), "{names:?}");
        assert!(names.contains(&"memory".to_string()), "{names:?}");
    }

    #[test]
    fn recorder_skipped_when_disabled_or_provider_read_only() {
        let dir = tempfile::tempdir().expect("temp dir");
        let store = sqlite_store(dir.path());
        let customizer = MemorySpawnCustomizer::new(
            store,
            AgentMemoryConfig {
                recorder_tool: false,
                ..AgentMemoryConfig::default()
            },
        );
        let mut spec = spec_for("agent:mem");
        customizer
            .apply(&MobId::from("test-mob"), None, &mut spec)
            .expect("apply succeeds");
        assert!(spec.external_tools.is_none(), "recorder_tool=false");

        // A custom recall-only provider has no authored-write support.
        let customizer =
            MemorySpawnCustomizer::new(Arc::new(RecallOnlyProvider), AgentMemoryConfig::default());
        let mut spec = spec_for("agent:mem");
        customizer
            .apply(&MobId::from("test-mob"), None, &mut spec)
            .expect("apply succeeds");
        assert!(
            spec.external_tools.is_none(),
            "read-only provider must not register the recorder"
        );
    }

    #[test]
    fn invalid_memory_identity_skips_without_failing_spawn() {
        let dir = tempfile::tempdir().expect("temp dir");
        let store = sqlite_store(dir.path());
        let customizer = MemorySpawnCustomizer::new(store, AgentMemoryConfig::default());

        // Whitespace fails the memory-scope identity validation.
        let mut spec = spec_for("agent with spaces");
        customizer
            .apply(&MobId::from("test-mob"), None, &mut spec)
            .expect("apply must not fail the spawn");
        assert!(spec.external_tools.is_none());
        assert!(spec.additional_instructions.is_none());
    }

    /// A member private to a person is given no memory record at its start:
    /// the records under its bare name are those of every member of that
    /// name, whoever it is private to. A member private to nobody is given
    /// them as before.
    #[tokio::test]
    async fn a_private_member_built_without_the_identity_layer_is_injected_no_record_of_its_bare_name()
     {
        let dir = tempfile::tempdir().expect("temp dir");
        let store = sqlite_store(dir.path());
        seed_record(
            &store,
            "ledger-side",
            "Deploy window",
            "Deploys are frozen on Fridays.",
        )
        .await;
        let customizer = MemorySpawnCustomizer::new(
            store.clone(),
            AgentMemoryConfig {
                selection: crate::identity_first::AgentMemorySelection::Always,
                ..AgentMemoryConfig::default()
            },
        );
        let injected = |spec: &SpawnMemberSpec| {
            spec.additional_instructions
                .as_ref()
                .is_some_and(|instructions| {
                    instructions
                        .iter()
                        .any(|section| section.contains("Deploys are frozen on Fridays."))
                })
        };

        let mut private = spec_for("ledger-side");
        private.labels = Some(std::collections::BTreeMap::from([(
            crate::access::PRIVATE_TO_LABEL.to_string(),
            "console-caller:v1:aa".to_string(),
        )]));
        customizer
            .apply(&MobId::from("test-mob"), None, &mut private)
            .expect("apply succeeds");
        assert!(
            !injected(&private),
            "a private member is given no record of its bare name: {:#?}",
            private.additional_instructions
        );

        let mut shared = spec_for("ledger-side");
        customizer
            .apply(&MobId::from("test-mob"), None, &mut shared)
            .expect("apply succeeds");
        assert!(
            injected(&shared),
            "a member private to nobody is given its name's records: {:#?}",
            shared.additional_instructions
        );
    }

    /// A member that a private member asks for carries no owner label yet
    /// when this customizer is asked: the mob's owner record says who it
    /// will be private to. It is given no record of its bare name, and its
    /// `memory` tool keeps its records under its owner and its name.
    #[tokio::test]
    async fn a_private_members_spawn_is_given_no_record_of_its_bare_name_before_its_labels_name_its_owner()
     {
        use crate::memory::capabilities::MemoryPanelStore as _;

        let dir = tempfile::tempdir().expect("temp dir");
        let store = sqlite_store(dir.path());
        seed_record(
            &store,
            "ledger-side",
            "Deploy window",
            "Deploys are frozen on Fridays.",
        )
        .await;
        let owners = crate::private_owners::PrivateOwners::default();
        owners.record(
            "test-mob",
            "side",
            Some(&std::collections::BTreeMap::from([(
                crate::access::PRIVATE_TO_LABEL.to_string(),
                "console-caller:v1:aa".to_string(),
            )])),
        );
        let customizer = MemorySpawnCustomizer::new(
            store.clone(),
            AgentMemoryConfig {
                selection: crate::identity_first::AgentMemorySelection::Always,
                ..AgentMemoryConfig::default()
            },
        )
        .with_private_owners(owners);

        let mut spec = spec_for("ledger-side");
        customizer
            .apply(
                &MobId::from("test-mob"),
                Some(&MobIdentity::from("side")),
                &mut spec,
            )
            .expect("apply succeeds");
        assert!(
            spec.additional_instructions
                .as_ref()
                .is_none_or(|instructions| instructions
                    .iter()
                    .all(|section| !section.contains("Deploys are frozen on Fridays."))),
            "the spawn is given no record of its bare name: {:#?}",
            spec.additional_instructions
        );

        let tools = spec.external_tools.as_ref().expect("recorder registered");
        let args = serde_json::value::RawValue::from_string(
            serde_json::json!({
                "action": "remember",
                "title": "Where the heron is",
                "body": "The heron is at the north pond.",
            })
            .to_string(),
        )
        .expect("tool arguments");
        let outcome = tools
            .dispatch(meerkat_core::types::ToolCallView {
                id: "call-1",
                name: crate::identity_first::MEMORY_TOOL_NAME,
                args: &args,
            })
            .await
            .expect("dispatch");
        assert!(!outcome.result.is_error, "the member stores its note");
        let kept: Vec<String> = store
            .records_page("default", None, None, None, 10, None)
            .await
            .expect("records")
            .records
            .into_iter()
            .filter(|record| record.body.contains("the north pond"))
            .map(|record| record.scope.key().to_string())
            .collect();
        assert_eq!(
            kept,
            vec!["private/console-caller:v1:aa/ledger-side".to_string()]
        );
    }
}
