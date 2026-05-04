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
    SkillKey, SkillName, SkillRuntime, SkillScope, SkillSource, SourceIdentityRecord,
    SourceIdentityRegistry, SourceIdentityStatus, SourceTransportKind, SourceUuid, apply_filter,
};
use meerkat_skills::{DefaultSkillEngine, InMemorySkillSource};
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
        Ok(apply_filter(&[self.doc.descriptor.clone()], filter))
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
        function_name: &str,
        arguments: serde_json::Value,
    ) -> Result<serde_json::Value, SkillError> {
        self.require_key(key)?;
        if function_name != "echo" {
            return Err(SkillError::Load(
                format!("missing function: {function_name}").into(),
            ));
        }
        Ok(serde_json::json!({ "received": arguments }))
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
    assert!(json["output"]["received"].is_null());
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
