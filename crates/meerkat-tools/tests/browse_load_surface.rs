#![allow(clippy::panic)]

//! Skill builtin tool surface tests (C-T §6 #15).
//!
//! Covers the agent-facing tools exposed by
//! `meerkat-tools::builtin::skills`. Post-V4 the surface is strictly typed:
//! every identifier on the wire is `SkillKey = (SourceUuid, SkillName)`, no
//! slash-path parsing.
//!
//! Cases:
//! - browse with no filter returns every listed skill.
//! - browse with `query` narrows by substring match (case-insensitive).
//! - browse with `source_uuid` filter narrows by source.
//! - browse rejects an unparseable `source_uuid`.
//! - browse over a composite with a key remap emits the canonical skill
//!   exactly once, matching the copy load serves.
//! - load returns the skill body + key fields for a known key.
//! - load surfaces `ExecutionFailed` for a missing key.
//! - load rejects a missing `source_uuid` / `skill_name` as
//!   `InvalidArgs`.
//!
//! See `docs/wave-c-prep/test-coverage-audit.md` §6 #15.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use indexmap::IndexMap;
use meerkat_core::skills::{
    SkillArtifact, SkillArtifactContent, SkillDescriptor, SkillDocument, SkillError, SkillFilter,
    SkillKey, SkillKeyRemap, SkillName, SkillRuntime, SkillScope, SkillSource,
    SourceIdentityLineage, SourceIdentityLineageEvent, SourceIdentityRecord,
    SourceIdentityRegistry, SourceIdentityStatus, SourceTransportKind, SourceUuid, apply_filter,
};
use meerkat_skills::source::SourceNode;
use meerkat_skills::{CompositeSkillSource, DefaultSkillEngine, InMemorySkillSource, NamedSource};
use meerkat_tools::BuiltinTool;
use meerkat_tools::builtin::skills::{
    BrowseSkillsTool, LoadSkillTool, SkillInvokeFunctionTool, SkillListResourcesTool,
    SkillReadResourceTool,
};

fn skill_doc(source: &SourceUuid, skill: &str, display: &str, body: &str) -> SkillDocument {
    SkillDocument {
        descriptor: SkillDescriptor {
            key: SkillKey {
                source_uuid: source.clone(),
                skill_name: SkillName::parse(skill).unwrap(),
            },
            name: display.to_string(),
            description: format!("description for {display}"),
            scope: SkillScope::Project,
            metadata: IndexMap::new(),
            capability_requirements: Vec::new(),
            source_name: "project".to_string(),
        },
        body: body.to_string(),
        extensions: IndexMap::new(),
    }
}

fn fixture_runtime() -> (Arc<SkillRuntime>, SourceUuid, SourceUuid) {
    let primary = SourceUuid::parse("dc256086-0d2f-4f61-a307-320d4148107f").unwrap();
    let secondary = SourceUuid::parse("fe52aa61-1111-4a22-9999-bbbbbbbbbbbb").unwrap();

    let docs = vec![
        skill_doc(&primary, "alpha", "Alpha", "# Alpha\n\nbody-alpha"),
        skill_doc(&primary, "beta", "Beta", "# Beta\n\nbody-beta"),
        skill_doc(
            &secondary,
            "gamma",
            "Gamma Helper",
            "# Gamma Helper\n\nbody-gamma",
        ),
    ];

    let source = InMemorySkillSource::new(docs);
    let registry = SourceIdentityRegistry::build(
        vec![
            SourceIdentityRecord {
                source_uuid: primary.clone(),
                display_name: "primary fixture".to_string(),
                transport_kind: SourceTransportKind::Filesystem,
                fingerprint: format!("fixture:{primary}"),
                status: SourceIdentityStatus::Active,
            },
            SourceIdentityRecord {
                source_uuid: secondary.clone(),
                display_name: "secondary fixture".to_string(),
                transport_kind: SourceTransportKind::Filesystem,
                fingerprint: format!("fixture:{secondary}"),
                status: SourceIdentityStatus::Active,
            },
        ],
        Vec::new(),
        Vec::new(),
        Vec::new(),
    )
    .unwrap();
    let engine = DefaultSkillEngine::new(source, Vec::new())
        .with_source_identity_registry(Arc::new(registry));
    let runtime = Arc::new(SkillRuntime::new(Arc::new(engine)));
    (runtime, primary, secondary)
}

fn runtime_with_unknown_source_identity() -> (Arc<SkillRuntime>, SourceUuid) {
    let unknown = SourceUuid::parse("aaaaaaaa-aaaa-4aaa-aaaa-aaaaaaaaaaaa").unwrap();
    let source = InMemorySkillSource::new(vec![skill_doc(
        &unknown,
        "orphaned",
        "Orphaned",
        "# Orphaned\n\nbody-orphaned",
    )]);
    let engine = DefaultSkillEngine::new(source, Vec::new())
        .with_source_identity_registry(Arc::new(SourceIdentityRegistry::default()));
    (Arc::new(SkillRuntime::new(Arc::new(engine))), unknown)
}

/// Composite fixture mirroring the engine remap fixture: a legacy source and
/// a canonical source both hosting `alpha`, with a source-identity remap
/// `legacy/alpha -> canonical/alpha`. Returns the runtime plus both keys.
fn remap_composite_runtime() -> (Arc<SkillRuntime>, SkillKey, SkillKey) {
    let legacy_source = SourceUuid::parse("aaaaaaaa-aaaa-4aaa-aaaa-aaaaaaaaaaaa").unwrap();
    let canonical_source = SourceUuid::parse("bbbbbbbb-bbbb-4bbb-bbbb-bbbbbbbbbbbb").unwrap();
    let legacy_key = SkillKey {
        source_uuid: legacy_source.clone(),
        skill_name: SkillName::parse("alpha").unwrap(),
    };
    let canonical_key = SkillKey {
        source_uuid: canonical_source.clone(),
        skill_name: SkillName::parse("alpha").unwrap(),
    };
    let record = |source_uuid: &SourceUuid, name: &str| SourceIdentityRecord {
        source_uuid: source_uuid.clone(),
        display_name: name.to_string(),
        transport_kind: SourceTransportKind::Filesystem,
        fingerprint: format!("fixture:{name}"),
        status: SourceIdentityStatus::Active,
    };
    let registry = Arc::new(
        SourceIdentityRegistry::build(
            vec![
                record(&legacy_source, "legacy"),
                record(&canonical_source, "canonical"),
            ],
            vec![SourceIdentityLineage {
                event_id: "legacy-to-canonical".to_string(),
                recorded_at_unix_secs: 1,
                required_from_skills: Vec::new(),
                event: SourceIdentityLineageEvent::RenameOrRelocate {
                    from: legacy_source.clone(),
                    to: canonical_source.clone(),
                },
            }],
            vec![SkillKeyRemap {
                from: legacy_key.clone(),
                to: canonical_key.clone(),
                reason: Some("test remap".to_string()),
            }],
            Vec::new(),
        )
        .unwrap(),
    );
    let composite = CompositeSkillSource::from_named_with_registry(
        vec![
            NamedSource::new(
                record(&legacy_source, "legacy"),
                SourceNode::Memory(InMemorySkillSource::new(vec![skill_doc(
                    &legacy_source,
                    "alpha",
                    "Legacy Alpha",
                    "# Legacy Alpha\n\nbody-legacy",
                )])),
            ),
            NamedSource::new(
                record(&canonical_source, "canonical"),
                SourceNode::Memory(InMemorySkillSource::new(vec![skill_doc(
                    &canonical_source,
                    "alpha",
                    "Canonical Alpha",
                    "# Canonical Alpha\n\nbody-canonical",
                )])),
            ),
        ],
        Arc::clone(&registry),
    );
    let engine =
        DefaultSkillEngine::new(composite, Vec::new()).with_source_identity_registry(registry);
    (
        Arc::new(SkillRuntime::new(Arc::new(engine))),
        legacy_key,
        canonical_key,
    )
}

struct HarnessSkillSource {
    doc: SkillDocument,
}

impl HarnessSkillSource {
    fn new(source: &SourceUuid) -> Self {
        Self {
            doc: skill_doc(source, "alpha", "Alpha", "# Alpha\n\nbody-alpha"),
        }
    }

    fn artifact(&self) -> SkillArtifact {
        SkillArtifact {
            path: "README.md".to_string(),
            mime_type: "text/markdown".to_string(),
            byte_length: 12,
        }
    }

    fn require_key(&self, key: &SkillKey) -> Result<(), SkillError> {
        if key == &self.doc.descriptor.key {
            Ok(())
        } else {
            Err(SkillError::NotFound { key: key.clone() })
        }
    }
}

impl SkillSource for HarnessSkillSource {
    async fn list(&self, filter: &SkillFilter) -> Result<Vec<SkillDescriptor>, SkillError> {
        Ok(apply_filter(
            std::slice::from_ref(&self.doc.descriptor),
            filter,
        ))
    }

    async fn load(&self, key: &SkillKey) -> Result<SkillDocument, SkillError> {
        self.require_key(key)?;
        Ok(self.doc.clone())
    }

    async fn list_artifacts(&self, key: &SkillKey) -> Result<Vec<SkillArtifact>, SkillError> {
        self.require_key(key)?;
        Ok(vec![self.artifact()])
    }

    async fn read_artifact(
        &self,
        key: &SkillKey,
        artifact_path: &str,
    ) -> Result<SkillArtifactContent, SkillError> {
        self.require_key(key)?;
        if artifact_path != "README.md" {
            return Err(SkillError::Load(
                format!("missing artifact: {artifact_path}").into(),
            ));
        }
        Ok(SkillArtifactContent {
            path: artifact_path.to_string(),
            mime_type: "text/markdown".to_string(),
            content: "artifact body".to_string(),
        })
    }

    async fn invoke_function(
        &self,
        key: &SkillKey,
        function_name: &meerkat_core::skills::SkillFunctionName,
        arguments: meerkat_core::ToolCallArguments,
    ) -> Result<meerkat_core::skills::SkillFunctionOutput, SkillError> {
        self.require_key(key)?;
        if function_name.as_str() != "echo" {
            return Err(SkillError::Load(
                format!("missing function: {function_name}").into(),
            ));
        }
        meerkat_core::skills::SkillFunctionOutput::from_serialize(
            &serde_json::json!({ "received": arguments.as_value() }),
        )
        .map_err(|err| SkillError::Load(format!("serialize echo output: {err}").into()))
    }
}

fn harness_runtime() -> (Arc<SkillRuntime>, SourceUuid) {
    let source = SourceUuid::parse("dc256086-0d2f-4f61-a307-320d4148107f").unwrap();
    let registry = SourceIdentityRegistry::build(
        vec![SourceIdentityRecord {
            source_uuid: source.clone(),
            display_name: "harness fixture".to_string(),
            transport_kind: SourceTransportKind::Filesystem,
            fingerprint: format!("fixture:{source}"),
            status: SourceIdentityStatus::Active,
        }],
        Vec::new(),
        Vec::new(),
        Vec::new(),
    )
    .unwrap();
    let engine = DefaultSkillEngine::new(HarnessSkillSource::new(&source), Vec::new())
        .with_source_identity_registry(Arc::new(registry));
    (Arc::new(SkillRuntime::new(Arc::new(engine))), source)
}

#[tokio::test]
async fn browse_no_filter_returns_all_skills() {
    let (runtime, _primary, _secondary) = fixture_runtime();
    let tool = BrowseSkillsTool::new(runtime);

    // Pre-condition: tool is named and has a definition.
    assert_eq!(tool.name(), "browse_skills");
    let def = tool.def();
    assert_eq!(def.name, "browse_skills");

    let output = tool.call(serde_json::json!({})).await.unwrap();
    let json = output.into_json().unwrap();
    let skills = json["skills"].as_array().unwrap();
    assert_eq!(skills.len(), 3, "no filter = all skills; got {json}");

    let names: Vec<&str> = skills
        .iter()
        .map(|s| s["skill_name"].as_str().unwrap())
        .collect();
    // BTreeMap ordering inside InMemorySkillSource → deterministic order.
    assert!(names.contains(&"alpha"));
    assert!(names.contains(&"beta"));
    assert!(names.contains(&"gamma"));
}

#[tokio::test]
async fn browse_query_filter_narrows_case_insensitive() {
    let (runtime, _p, _s) = fixture_runtime();
    let tool = BrowseSkillsTool::new(runtime);

    let output = tool
        .call(serde_json::json!({ "query": "HELPER" }))
        .await
        .unwrap();
    let json = output.into_json().unwrap();
    let skills = json["skills"].as_array().unwrap();
    assert_eq!(
        skills.len(),
        1,
        "only gamma's description mentions 'Helper'; got {json}"
    );
    assert_eq!(skills[0]["skill_name"].as_str().unwrap(), "gamma");
}

#[tokio::test]
async fn browse_source_uuid_filter_narrows_by_source() {
    let (runtime, _primary, secondary) = fixture_runtime();
    let tool = BrowseSkillsTool::new(runtime);

    let output = tool
        .call(serde_json::json!({ "source_uuid": secondary.to_string() }))
        .await
        .unwrap();
    let json = output.into_json().unwrap();
    let skills = json["skills"].as_array().unwrap();
    assert_eq!(skills.len(), 1, "only secondary has one skill");
    assert_eq!(skills[0]["skill_name"].as_str().unwrap(), "gamma");
    assert_eq!(
        skills[0]["source_uuid"].as_str().unwrap(),
        secondary.to_string()
    );
}

#[tokio::test]
async fn browse_rejects_unparseable_source_uuid() {
    let (runtime, _p, _s) = fixture_runtime();
    let tool = BrowseSkillsTool::new(runtime);

    let err = tool
        .call(serde_json::json!({ "source_uuid": "not-a-uuid" }))
        .await
        .unwrap_err();

    match &err {
        meerkat_tools::BuiltinToolError::ExecutionFailed(msg) => {
            assert!(
                msg.to_lowercase().contains("uuid"),
                "ExecutionFailed message must name the parse failure; got {msg:?}",
            );
        }
        other => panic!("expected ExecutionFailed for bad uuid, got {other:?}"),
    }
}

#[tokio::test]
async fn browse_surfaces_source_identity_registry_failures() {
    let (runtime, unknown) = runtime_with_unknown_source_identity();
    let tool = BrowseSkillsTool::new(runtime);

    let err = tool.call(serde_json::json!({})).await.unwrap_err();

    match &err {
        meerkat_tools::BuiltinToolError::ExecutionFailed(msg) => {
            assert!(
                msg.contains("source identity resolution failed"),
                "browse failure must name source identity resolution; got {msg:?}",
            );
            assert!(
                msg.contains(&unknown.to_string()),
                "browse failure must identify the failing source; got {msg:?}",
            );
            assert!(
                msg.contains("source unknown"),
                "browse failure must include the registry failure; got {msg:?}",
            );
        }
        other => panic!("expected ExecutionFailed for source identity failure, got {other:?}"),
    }
}

/// Browse over a composite with a key remap must emit the canonical skill
/// EXACTLY ONCE, advertising the copy `load` actually serves (the canonical
/// host), never the remapped-away duplicate.
#[tokio::test]
async fn browse_emits_remapped_canonical_skill_once_matching_load() {
    let (runtime, legacy_key, canonical_key) = remap_composite_runtime();
    let browse = BrowseSkillsTool::new(Arc::clone(&runtime));

    let output = browse.call(serde_json::json!({})).await.unwrap();
    let json = output.into_json().unwrap();
    let skills = json["skills"].as_array().unwrap();

    assert_eq!(
        skills.len(),
        1,
        "the remapped canonical skill must browse exactly once; got {json}"
    );
    let entry = &skills[0];
    assert_eq!(
        entry["source_uuid"].as_str().unwrap(),
        canonical_key.source_uuid.to_string(),
        "browse must advertise the canonical host key; got {json}"
    );
    assert_eq!(
        entry["skill_name"].as_str().unwrap(),
        canonical_key.skill_name.as_str()
    );
    assert_eq!(
        entry["name"].as_str().unwrap(),
        "Canonical Alpha",
        "browse must carry the loadable copy's display name; got {json}"
    );
    assert_eq!(
        entry["description"].as_str().unwrap(),
        "description for Canonical Alpha",
        "browse must carry the loadable copy's description; got {json}"
    );

    // Load through the remapped-away key: the runtime must serve exactly the
    // copy browse advertised.
    let load = LoadSkillTool::new(runtime);
    let loaded = load
        .call(serde_json::json!({
            "source_uuid": legacy_key.source_uuid.to_string(),
            "skill_name": legacy_key.skill_name.as_str(),
        }))
        .await
        .unwrap();
    let loaded = loaded.into_json().unwrap();

    assert_eq!(
        loaded["source_uuid"], entry["source_uuid"],
        "browse and load must agree on the served key; got browse={entry} load={loaded}"
    );
    assert_eq!(loaded["skill_name"], entry["skill_name"]);
    assert_eq!(
        loaded["name"], entry["name"],
        "browse and load must agree on the served copy; got browse={entry} load={loaded}"
    );
    assert!(
        loaded["body"].as_str().unwrap().contains("body-canonical"),
        "load must serve the canonical host body; got {loaded}"
    );
}

#[tokio::test]
async fn load_returns_body_and_key_fields_for_known_skill() {
    let (runtime, primary, _s) = fixture_runtime();
    let tool = LoadSkillTool::new(runtime);

    let output = tool
        .call(serde_json::json!({
            "source_uuid": primary.to_string(),
            "skill_name": "alpha",
        }))
        .await
        .unwrap();
    let json = output.into_json().unwrap();

    assert_eq!(json["skill_name"].as_str().unwrap(), "alpha");
    assert_eq!(json["source_uuid"].as_str().unwrap(), primary.to_string());
    let body = json["body"].as_str().unwrap();
    assert!(
        body.contains("body-alpha"),
        "rendered body must contain the source body; got {body:?}"
    );
    assert!(
        json["byte_size"].as_u64().unwrap() > 0,
        "byte_size must be nonzero for a real skill"
    );
}

#[tokio::test]
async fn load_missing_skill_surfaces_execution_failure() {
    let (runtime, primary, _s) = fixture_runtime();
    let tool = LoadSkillTool::new(runtime);

    let err = tool
        .call(serde_json::json!({
            "source_uuid": primary.to_string(),
            "skill_name": "ghost",
        }))
        .await
        .unwrap_err();

    match &err {
        meerkat_tools::BuiltinToolError::ExecutionFailed(_) => {}
        other => panic!("load of unknown skill must return ExecutionFailed; got {other:?}"),
    }
}

#[tokio::test]
async fn load_missing_required_args_returns_invalid_args() {
    let (runtime, _p, _s) = fixture_runtime();
    let tool = LoadSkillTool::new(runtime);

    let err = tool.call(serde_json::json!({})).await.unwrap_err();
    match &err {
        meerkat_tools::BuiltinToolError::InvalidArgs(msg) => {
            assert!(
                msg.contains("source_uuid"),
                "InvalidArgs must name the missing field; got {msg:?}"
            );
        }
        other => panic!("expected InvalidArgs for empty payload; got {other:?}"),
    }

    let err = tool
        .call(serde_json::json!({ "source_uuid": "dc256086-0d2f-4f61-a307-320d4148107f" }))
        .await
        .unwrap_err();
    match &err {
        meerkat_tools::BuiltinToolError::InvalidArgs(msg) => {
            assert!(
                msg.contains("skill_name"),
                "InvalidArgs must name the missing skill_name; got {msg:?}"
            );
        }
        other => panic!("expected InvalidArgs for missing skill_name; got {other:?}"),
    }
}

#[tokio::test]
async fn browse_output_always_carries_typed_key_fields() {
    let (runtime, _p, _s) = fixture_runtime();
    let tool = BrowseSkillsTool::new(runtime);

    let output = tool.call(serde_json::json!({})).await.unwrap();
    let json = output.into_json().unwrap();
    let skills = json["skills"].as_array().unwrap();

    // Each result must have typed key fields on every row — no legacy
    // slash path, no bare `id`. This is the V4 wire discipline check.
    for entry in skills {
        let obj = entry.as_object().expect("skill rows are objects");
        assert!(
            obj.contains_key("source_uuid"),
            "each row must carry source_uuid; got {entry}",
        );
        assert!(
            obj.contains_key("skill_name"),
            "each row must carry skill_name; got {entry}",
        );
        // Parse both halves to confirm they're typed-safe.
        SourceUuid::parse(obj["source_uuid"].as_str().unwrap()).unwrap();
        SkillName::parse(obj["skill_name"].as_str().unwrap()).unwrap();
    }
}

#[tokio::test]
async fn list_resources_returns_artifact_metadata_for_known_skill() {
    let (runtime, source) = harness_runtime();
    let tool = SkillListResourcesTool::new(runtime);

    let output = tool
        .call(serde_json::json!({
            "source_uuid": source.to_string(),
            "skill_name": "alpha",
        }))
        .await
        .unwrap();
    let json = output.into_json().unwrap();
    let artifacts = json["artifacts"].as_array().unwrap();

    assert_eq!(artifacts.len(), 1);
    assert_eq!(artifacts[0]["path"].as_str().unwrap(), "README.md");
    assert_eq!(artifacts[0]["mime_type"].as_str().unwrap(), "text/markdown");
}

#[tokio::test]
async fn read_resource_returns_artifact_content_for_known_path() {
    let (runtime, source) = harness_runtime();
    let tool = SkillReadResourceTool::new(runtime);

    let output = tool
        .call(serde_json::json!({
            "source_uuid": source.to_string(),
            "skill_name": "alpha",
            "path": "README.md",
        }))
        .await
        .unwrap();
    let json = output.into_json().unwrap();

    assert_eq!(json["artifact"]["path"].as_str().unwrap(), "README.md");
    assert_eq!(
        json["artifact"]["content"].as_str().unwrap(),
        "artifact body"
    );
}

#[tokio::test]
async fn read_resource_missing_path_returns_invalid_args() {
    let (runtime, source) = harness_runtime();
    let tool = SkillReadResourceTool::new(runtime);

    let err = tool
        .call(serde_json::json!({
            "source_uuid": source.to_string(),
            "skill_name": "alpha",
        }))
        .await
        .unwrap_err();

    match &err {
        meerkat_tools::BuiltinToolError::InvalidArgs(msg) => {
            assert!(
                msg.contains("path"),
                "InvalidArgs must name the missing path; got {msg:?}"
            );
        }
        other => panic!("expected InvalidArgs for missing path; got {other:?}"),
    }
}

#[tokio::test]
async fn invoke_function_passes_typed_key_and_default_arguments() {
    let (runtime, source) = harness_runtime();
    let tool = SkillInvokeFunctionTool::new(runtime);

    let output = tool
        .call(serde_json::json!({
            "source_uuid": source.to_string(),
            "skill_name": "alpha",
            "function_name": "echo",
        }))
        .await
        .unwrap();
    let json = output.into_json().unwrap();

    assert_eq!(json["function_name"].as_str().unwrap(), "echo");
    // Default arguments are the canonical empty object — the typed
    // `ToolCallArguments` ingress never fabricates `null` arguments.
    assert_eq!(
        json["output"]["received"],
        serde_json::json!({}),
        "default arguments must be the empty object, got {json}"
    );
}

/// K1 regression: non-object `arguments` are rejected fail-closed at the tool
/// ingress (typed `ToolCallArguments` parse), never ferried to the engine.
#[tokio::test]
async fn invoke_function_rejects_non_object_arguments() {
    let (runtime, source) = harness_runtime();
    let tool = SkillInvokeFunctionTool::new(runtime);

    let err = tool
        .call(serde_json::json!({
            "source_uuid": source.to_string(),
            "skill_name": "alpha",
            "function_name": "echo",
            "arguments": "not-an-object",
        }))
        .await
        .unwrap_err();
    match &err {
        meerkat_tools::BuiltinToolError::InvalidArgs(msg) => {
            assert!(
                msg.contains("object"),
                "rejection must name the object-shape contract; got {msg:?}"
            );
        }
        other => panic!("expected InvalidArgs for non-object arguments; got {other:?}"),
    }
}

/// K1 regression: degenerate function identity is rejected at ingress by the
/// typed `SkillFunctionName` parse.
#[tokio::test]
async fn invoke_function_rejects_empty_function_name() {
    let (runtime, source) = harness_runtime();
    let tool = SkillInvokeFunctionTool::new(runtime);

    let err = tool
        .call(serde_json::json!({
            "source_uuid": source.to_string(),
            "skill_name": "alpha",
            "function_name": "  ",
        }))
        .await
        .unwrap_err();
    assert!(
        matches!(err, meerkat_tools::BuiltinToolError::InvalidArgs(_)),
        "whitespace function_name must fail typed at ingress; got {err:?}"
    );
}

#[tokio::test]
async fn invoke_function_preserves_structured_arguments() {
    let (runtime, source) = harness_runtime();
    let tool = SkillInvokeFunctionTool::new(runtime);

    let output = tool
        .call(serde_json::json!({
            "source_uuid": source.to_string(),
            "skill_name": "alpha",
            "function_name": "echo",
            "arguments": { "subject": "typed" },
        }))
        .await
        .unwrap();
    let json = output.into_json().unwrap();

    assert_eq!(
        json["output"]["received"]["subject"].as_str().unwrap(),
        "typed"
    );
}

// ---------------------------------------------------------------------------
// `skill_list_resources` and the "not found" answer.
//
// The `SkillSource::list_artifacts` default answers `SkillError::NotFound`
// for every key, so that answer alone does not say the skill is missing. The
// tool must not report a skill the skill listing shows as not found, must
// keep an empty listing a success, and must not reveal a skill the listing
// hides.
// ---------------------------------------------------------------------------

/// Whether `browse_skills`, filtered to the key's source, shows `key`: the
/// skill listing a caller of the skill tools sees.
async fn browse_shows(runtime: &Arc<SkillRuntime>, key: &SkillKey) -> bool {
    let json = BrowseSkillsTool::new(Arc::clone(runtime))
        .call(serde_json::json!({ "source_uuid": key.source_uuid.to_string() }))
        .await
        .unwrap()
        .into_json()
        .unwrap();
    let source_uuid = key.source_uuid.to_string();
    json["skills"].as_array().unwrap().iter().any(|skill| {
        skill["source_uuid"].as_str() == Some(source_uuid.as_str())
            && skill["skill_name"].as_str() == Some(key.skill_name.as_str())
    })
}

/// Row A: a skill the listing shows, held by a source that answers the
/// `list_artifacts` default (the in-memory source inherits it).
#[tokio::test]
async fn list_resources_of_a_listed_skill_with_no_resource_listing_does_not_say_not_found() {
    let (runtime, primary, _secondary) = fixture_runtime();
    let key = SkillKey {
        source_uuid: primary.clone(),
        skill_name: SkillName::parse("alpha").unwrap(),
    };

    // The skill listing shows the key.
    assert!(browse_shows(&runtime, &key).await);

    let err = SkillListResourcesTool::new(runtime)
        .call(serde_json::json!({
            "source_uuid": primary.to_string(),
            "skill_name": "alpha",
        }))
        .await
        .unwrap_err();
    // An error, not an empty listing: the call above did not succeed.
    let meerkat_tools::BuiltinToolError::ExecutionFailed(message) = &err else {
        panic!("a listed skill without a resource listing is an execution failure; got {err:?}");
    };

    let not_found = SkillError::NotFound { key: key.clone() }.to_string();
    assert!(
        !message.contains(&not_found),
        "a listed skill must not be reported as not found; got: {message}"
    );
    assert!(
        message.contains(&format!("The skill {key} is listed")),
        "the diagnostic names the key and says it is listed; got: {message}"
    );
    assert!(
        message.contains("no source returned a resource listing for it"),
        "the diagnostic says no source returned a resource listing; got: {message}"
    );
    assert!(
        message.contains("This is not an empty listing"),
        "the diagnostic says this is not an empty listing; got: {message}"
    );
    assert!(
        message.contains("If load_skill is available, call it with this key to use the skill."),
        "the diagnostic suggests one conditional next step; got: {message}"
    );
    // It states the two observations only: no source is named (the fixture's
    // source name is "project").
    assert!(
        !message.contains("project"),
        "the diagnostic names no source; got: {message}"
    );
}

/// A runtime whose one skill requires a capability the engine does not have,
/// so the skill listing hides it.
fn hidden_skill_runtime() -> (Arc<SkillRuntime>, SkillKey) {
    let source = SourceUuid::parse("dc256086-0d2f-4f61-a307-320d4148107f").unwrap();
    let mut doc = skill_doc(&source, "gated", "Gated", "# Gated\n\nbody-gated");
    doc.descriptor.capability_requirements =
        vec![meerkat_core::skills::CapabilityId::parse("shell").unwrap()];
    let key = doc.descriptor.key.clone();
    let registry = SourceIdentityRegistry::build(
        vec![SourceIdentityRecord {
            source_uuid: source.clone(),
            display_name: "gated fixture".to_string(),
            transport_kind: SourceTransportKind::Filesystem,
            fingerprint: format!("fixture:{source}"),
            status: SourceIdentityStatus::Active,
        }],
        Vec::new(),
        Vec::new(),
        Vec::new(),
    )
    .unwrap();
    // No capability is available to this engine.
    let engine = DefaultSkillEngine::new(InMemorySkillSource::new(vec![doc]), Vec::new())
        .with_source_identity_registry(Arc::new(registry));
    (Arc::new(SkillRuntime::new(Arc::new(engine))), key)
}

/// Row E: a skill the skill listing hides from this caller stays not found,
/// byte for byte, although a source holds it: the diagnostic of row A must
/// never reveal it.
#[tokio::test]
async fn list_resources_of_a_skill_the_listing_hides_stays_not_found() {
    let (runtime, key) = hidden_skill_runtime();

    // The skill listing does not show the key.
    assert!(!browse_shows(&runtime, &key).await);

    let err = SkillListResourcesTool::new(runtime)
        .call(serde_json::json!({
            "source_uuid": key.source_uuid.to_string(),
            "skill_name": key.skill_name.as_str(),
        }))
        .await
        .unwrap_err();
    let meerkat_tools::BuiltinToolError::ExecutionFailed(message) = &err else {
        panic!("a hidden skill is an execution failure; got {err:?}");
    };
    assert_eq!(message, &SkillError::NotFound { key }.to_string());
}

/// A source that holds one skill and whose listing fails. It inherits the
/// `list_artifacts` default.
struct FailingListSource {
    doc: SkillDocument,
}

impl SkillSource for FailingListSource {
    async fn list(&self, _filter: &SkillFilter) -> Result<Vec<SkillDescriptor>, SkillError> {
        Err(SkillError::Load(
            "fixture: the listing cannot be read".into(),
        ))
    }

    async fn load(&self, key: &SkillKey) -> Result<SkillDocument, SkillError> {
        if key == &self.doc.descriptor.key {
            Ok(self.doc.clone())
        } else {
            Err(SkillError::NotFound { key: key.clone() })
        }
    }
}

/// Row F: when the skill listing itself cannot be read, the answer stays not
/// found, byte for byte: a fact that could not be read is not taken as
/// "listed", even for a key the source holds.
#[tokio::test]
async fn list_resources_stays_not_found_when_the_skill_listing_fails() {
    let source = SourceUuid::parse("dc256086-0d2f-4f61-a307-320d4148107f").unwrap();
    let doc = skill_doc(&source, "alpha", "Alpha", "# Alpha\n\nbody-alpha");
    let key = doc.descriptor.key.clone();
    let registry = SourceIdentityRegistry::build(
        vec![SourceIdentityRecord {
            source_uuid: source.clone(),
            display_name: "failing listing fixture".to_string(),
            transport_kind: SourceTransportKind::Filesystem,
            fingerprint: format!("fixture:{source}"),
            status: SourceIdentityStatus::Active,
        }],
        Vec::new(),
        Vec::new(),
        Vec::new(),
    )
    .unwrap();
    let engine = DefaultSkillEngine::new(FailingListSource { doc }, Vec::new())
        .with_source_identity_registry(Arc::new(registry));
    let runtime = Arc::new(SkillRuntime::new(Arc::new(engine)));

    // The skill listing fails, for the engine and for `browse_skills`.
    let filter = SkillFilter {
        source_uuid: Some(source.clone()),
        ..SkillFilter::default()
    };
    assert!(runtime.list_all_with_provenance(&filter).await.is_err());
    assert!(
        BrowseSkillsTool::new(Arc::clone(&runtime))
            .call(serde_json::json!({ "source_uuid": source.to_string() }))
            .await
            .is_err()
    );
    // The resource listing itself answers the typed not found.
    assert!(matches!(
        runtime.list_artifacts(&key).await,
        Err(SkillError::NotFound { .. })
    ));

    let err = SkillListResourcesTool::new(runtime)
        .call(serde_json::json!({
            "source_uuid": key.source_uuid.to_string(),
            "skill_name": key.skill_name.as_str(),
        }))
        .await
        .unwrap_err();
    let meerkat_tools::BuiltinToolError::ExecutionFailed(message) = &err else {
        panic!("a failed skill listing is an execution failure; got {err:?}");
    };
    assert_eq!(message, &SkillError::NotFound { key }.to_string());
}

/// Row B: a key no source holds stays not found, typed at the runtime and in
/// the tool's text.
#[tokio::test]
async fn list_resources_of_a_missing_key_stays_not_found() {
    let (runtime, primary, _secondary) = fixture_runtime();
    let key = SkillKey {
        source_uuid: primary.clone(),
        skill_name: SkillName::parse("ghost").unwrap(),
    };

    match runtime.list_artifacts(&key).await {
        Err(SkillError::NotFound { key: missing }) => assert_eq!(missing, key),
        other => panic!("a missing key is SkillError::NotFound; got {other:?}"),
    }

    let err = SkillListResourcesTool::new(runtime)
        .call(serde_json::json!({
            "source_uuid": primary.to_string(),
            "skill_name": "ghost",
        }))
        .await
        .unwrap_err();
    let meerkat_tools::BuiltinToolError::ExecutionFailed(message) = &err else {
        panic!("a missing key is an execution failure; got {err:?}");
    };
    assert_eq!(message, &SkillError::NotFound { key }.to_string());
}

/// The embedded skill row C's later source serves. Registered in this test
/// binary only; no other case here reads the embedded source.
const LATER_SOURCE_SKILL: &str = "listed-by-a-later-source";
const LATER_SOURCE_REFERENCE: &str = "# Guide\n\nserved by the embedded source";

inventory::submit! {
    meerkat_skills::SkillRegistration {
        id: LATER_SOURCE_SKILL,
        name: "Listed By A Later Source",
        description: "fixture: an embedded skill with one reference document",
        scope: SkillScope::Builtin,
        requires_capabilities: &[],
        body: "# Listed By A Later Source\n\nbody-embedded",
        extensions: &[("references/guide.md", LATER_SOURCE_REFERENCE)],
    }
}

/// A composite whose first source holds the skill and inherits the
/// `list_artifacts` default, and whose later source (embedded) holds the same
/// key and lists its artifacts.
fn first_cannot_list_later_can_composite()
-> (CompositeSkillSource, Arc<SourceIdentityRegistry>, SkillKey) {
    let builtin = SourceUuid::builtin();
    let key = SkillKey::builtin(SkillName::parse(LATER_SOURCE_SKILL).unwrap());
    let record = |name: &str, transport_kind: SourceTransportKind| SourceIdentityRecord {
        source_uuid: builtin.clone(),
        display_name: name.to_string(),
        transport_kind,
        fingerprint: format!("fixture:{name}"),
        status: SourceIdentityStatus::Active,
    };
    let registry = Arc::new(
        SourceIdentityRegistry::build(
            vec![record("later-can-list", SourceTransportKind::Embedded)],
            Vec::new(),
            Vec::new(),
            Vec::new(),
        )
        .unwrap(),
    );
    let composite = CompositeSkillSource::from_named_with_registry(
        vec![
            NamedSource::new(
                record("first-cannot-list", SourceTransportKind::Filesystem),
                SourceNode::Memory(InMemorySkillSource::new(vec![skill_doc(
                    &builtin,
                    LATER_SOURCE_SKILL,
                    "First Copy",
                    "# First Copy\n\nbody-first",
                )])),
            ),
            NamedSource::new(
                record("later-can-list", SourceTransportKind::Embedded),
                SourceNode::Embedded(meerkat_skills::EmbeddedSkillSource::new()),
            ),
        ],
        Arc::clone(&registry),
    );
    (composite, registry, key)
}

/// Row C: a source that holds the skill and returns no resource listing does
/// not stop a later source that holds it and returns one.
#[tokio::test]
async fn list_resources_reaches_a_later_source_that_can_list_past_one_that_cannot() {
    let (composite, registry, key) = first_cannot_list_later_can_composite();
    let expected = SkillArtifact {
        path: "references/guide.md".to_string(),
        mime_type: "text/markdown".to_string(),
        byte_length: LATER_SOURCE_REFERENCE.len(),
    };

    // The first source holds the skill (it serves the load) and has no
    // resource listing of its own.
    let loaded = composite.load(&key).await.unwrap();
    assert_eq!(loaded.descriptor.source_name, "first-cannot-list");
    let first_alone = InMemorySkillSource::new(vec![skill_doc(
        &SourceUuid::builtin(),
        LATER_SOURCE_SKILL,
        "First Copy",
        "# First Copy\n\nbody-first",
    )]);
    assert!(first_alone.load(&key).await.is_ok());
    assert!(first_alone.list_artifacts(&key).await.is_err());

    // The composite answers with the later source's artifacts.
    assert_eq!(
        composite.list_artifacts(&key).await.unwrap(),
        vec![expected.clone()]
    );

    // And so does the tool over the engine.
    let engine =
        DefaultSkillEngine::new(composite, Vec::new()).with_source_identity_registry(registry);
    let runtime = Arc::new(SkillRuntime::new(Arc::new(engine)));
    let json = SkillListResourcesTool::new(runtime)
        .call(serde_json::json!({
            "source_uuid": key.source_uuid.to_string(),
            "skill_name": key.skill_name.as_str(),
        }))
        .await
        .unwrap()
        .into_json()
        .unwrap();
    assert_eq!(
        json["artifacts"],
        serde_json::to_value(vec![expected]).unwrap()
    );
}

/// The embedded skill of row D: its source lists artifacts and it has none.
/// Registered in this test binary only.
const NO_RESOURCES_SKILL: &str = "listing-source-no-resources";

inventory::submit! {
    meerkat_skills::SkillRegistration {
        id: NO_RESOURCES_SKILL,
        name: "Listing Source No Resources",
        description: "fixture: an embedded skill without a reference document",
        scope: SkillScope::Builtin,
        requires_capabilities: &[],
        body: "# Listing Source No Resources\n\nbody-embedded",
        extensions: &[],
    }
}

/// Row D: a source that lists artifacts and has none for an existing skill
/// answers a successful empty list: not an error, and not the diagnostic of
/// row A.
#[tokio::test]
async fn list_resources_of_an_existing_skill_with_no_resources_is_an_empty_success() {
    let key = SkillKey::builtin(SkillName::parse(NO_RESOURCES_SKILL).unwrap());
    // The engine's default registry holds the builtin source.
    let engine = DefaultSkillEngine::new(meerkat_skills::EmbeddedSkillSource::new(), Vec::new());
    let runtime = Arc::new(SkillRuntime::new(Arc::new(engine)));
    let args = serde_json::json!({
        "source_uuid": key.source_uuid.to_string(),
        "skill_name": key.skill_name.as_str(),
    });

    // The skill exists: the same key loads.
    let loaded = LoadSkillTool::new(Arc::clone(&runtime))
        .call(args.clone())
        .await
        .unwrap()
        .into_json()
        .unwrap();
    assert_eq!(loaded["skill_name"].as_str().unwrap(), NO_RESOURCES_SKILL);

    assert_eq!(
        runtime.list_artifacts(&key).await.unwrap(),
        Vec::<SkillArtifact>::new()
    );

    let json = SkillListResourcesTool::new(runtime)
        .call(args)
        .await
        .unwrap()
        .into_json()
        .unwrap();
    assert_eq!(json["artifacts"], serde_json::json!([]));
    assert_eq!(json["skill_name"].as_str().unwrap(), NO_RESOURCES_SKILL);
}
