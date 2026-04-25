#![allow(clippy::panic)]

//! Browse / load skill tool surface tests (C-T §6 #15).
//!
//! Covers the agent-facing tools `browse_skills` + `load_skill` exposed
//! by `meerkat-tools::builtin::skills`. Post-V4 the surface is strictly
//! typed: every identifier on the wire is `SkillKey = (SourceUuid,
//! SkillName)`, no slash-path parsing. These tests ride the real
//! `SkillRuntime` over an `InMemorySkillSource` so the BuiltinTool
//! contract (def / call / output shape) is exercised end-to-end.
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
    SkillDescriptor, SkillDocument, SkillKey, SkillName, SkillRuntime, SkillScope, SourceUuid,
};
use meerkat_skills::{DefaultSkillEngine, InMemorySkillSource};
use meerkat_tools::BuiltinTool;
use meerkat_tools::builtin::skills::{BrowseSkillsTool, LoadSkillTool};

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
    let engine = DefaultSkillEngine::new(source, Vec::new());
    let runtime = Arc::new(SkillRuntime::new(Arc::new(engine)));
    (runtime, primary, secondary)
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
