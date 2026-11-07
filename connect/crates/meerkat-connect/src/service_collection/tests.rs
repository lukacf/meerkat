//! Contract/schema controls only. Synthetic ToolResult bodies here exercise a
//! trusted schema program; they are never native receipts or collection proof.
use super::*;
use crate::service_probe::{ProbeStatus, ResourceAssessment};
use serde_json::json;

struct Pages;
struct Cursor {
    page: u64,
    resource: String,
}
impl CollectionProgram for Pages {
    fn accepts_target(&self, target: &CollectionTarget<'_>) -> bool {
        target.resource()["repository"]
            .as_str()
            .is_some_and(|v| !v.is_empty())
            && target.installed_connection().name == "selected"
            && target.raw_operation_ceiling() == &["list".to_owned()].into_iter().collect()
    }
    fn start(&self, target: &CollectionTarget<'_>) -> Box<dyn CollectionCursor> {
        Box::new(Cursor {
            page: 1,
            resource: target.resource()["repository"].as_str().unwrap().into(),
        })
    }
}
impl CollectionCursor for Cursor {
    fn begin(&mut self) -> CollectionStep {
        CollectionStep::Call(
            CollectionCall::new(
                "list".into(),
                json!({"repository":self.resource,"page":self.page}),
            )
            .unwrap(),
        )
    }
    fn observe(&mut self, result: &CommittedCollectionResult<'_>) -> CollectionStep {
        let Ok(body) = serde_json::from_str::<Value>(&result.result().text_content()) else {
            return CollectionStep::Finish(ResourceAssessment::Unknown);
        };
        if body["repository"] != self.resource || body["page"] != self.page {
            return CollectionStep::Finish(ResourceAssessment::NotVerified);
        }
        match body.get("next") {
            Some(Value::Null) => CollectionStep::Finish(ResourceAssessment::Verified),
            Some(next) if next.as_u64() == Some(self.page + 1) => {
                self.page += 1;
                self.begin()
            }
            _ => CollectionStep::Finish(ResourceAssessment::Unknown),
        }
    }
}
pub(super) fn requirement(id: &str, repository: &str) -> RequiredCollectionOperation {
    RequiredCollectionOperation::new(
        id.into(),
        json!({"repository":repository}),
        vec!["list".into()],
        Arc::new(Pages),
    )
    .unwrap()
}
fn selected() -> McpServerConfig {
    serde_json::from_value(json!({"name":"selected","command":"/bin/false","args":[]})).unwrap()
}
fn schema_observation<'a>(
    target: CollectionTarget<'a>,
    arguments: &'a Value,
    result: &'a ToolResult,
    native: &'a ServiceProbeReport,
) -> CommittedCollectionResult<'a> {
    CommittedCollectionResult {
        target,
        operation: "list",
        arguments,
        result,
        native,
        read_observation: None,
        invocation: CollectionInvocationObservation(Arc::new(())),
    }
}
#[test]
fn read_request_is_only_intent_and_schema_observation_has_no_handle() {
    let ordinary = CollectionCall::new("list".into(), json!({})).unwrap();
    let read = CollectionCall::read_only("list".into(), json!({})).unwrap();
    assert!(!ordinary.read_only);
    assert!(read.read_only);
    assert!(CollectionCall::read_only("list".into(), json!(null)).is_err());
    let r = requirement("one", "one");
    let installed = selected();
    let native = unqualified_schema_report();
    let body = ToolResult::new("schema-only".into(), "{}".into(), false);
    let target = CollectionTarget {
        source_id: "source",
        requirement_id: &r.id,
        resource: &r.resource,
        operations: &r.operations,
        installed: &installed,
    };
    assert!(
        schema_observation(target, &read.arguments, &body, &native)
            .read_observation()
            .is_none()
    );
}
fn unqualified_schema_report() -> ServiceProbeReport {
    ServiceProbeReport {
        profile_id: "schema-only".into(),
        application_id: "fixture".into(),
        instance_id: "fixture".into(),
        login_scope_id: "fixture".into(),
        viewer_id: "fixture".into(),
        origin: "https://invalid.example".into(),
        realm: "fixture".into(),
        session_id: meerkat_core::SessionId::new(),
        job_id: "schema-only".into(),
        server: "selected".into(),
        agent: "fixture".into(),
        operation: "list".into(),
        resource: json!({"repository":"one"}),
        status: ProbeStatus::NotObserved,
        input_id: None,
        run_id: None,
        tool_call_id: None,
        result_sha256: None,
        installed_connection_sha256: None,
        dispatch_outcome: None,
        native_terminal_committed: false,
        native_close_joined: false,
    }
}
#[test]
fn collection_keeps_exact_sources_and_required_resources_without_feature_deduplication() {
    let make = |source: &str| {
        RequiredServiceCollection::new(
            source.into(),
            "family".into(),
            "profile".into(),
            "agent".into(),
            vec![
                requirement("read-one", "one"),
                requirement("read-two", "two"),
            ],
            CollectionLimits::new(4, 4096, 4096).unwrap(),
        )
        .unwrap()
    };
    let a = make("source-a");
    let b = make("source-b");
    assert_ne!(a.source_id(), b.source_id());
    assert_eq!(
        a.required_operation_ids().collect::<Vec<_>>(),
        vec!["read-one", "read-two"]
    );
    assert_eq!(a.requirements[0].resource, json!({"repository":"one"}));
    assert_eq!(a.requirements[1].resource, json!({"repository":"two"}));
    assert!(
        RequiredServiceCollection::new(
            "source".into(),
            "f".into(),
            "p".into(),
            "a".into(),
            vec![requirement("same", "one"), requirement("same", "two")],
            CollectionLimits::new(2, 4096, 4096).unwrap()
        )
        .is_err()
    );
}
#[test]
fn collection_rejects_unbounded_empty_and_duplicate_operation_intent() {
    for limits in [
        (0, 1, 1),
        (257, 1, 1),
        (1, 0, 1),
        (1, 1, 0),
        (1, 8 * 1024 * 1024 + 1, 1),
        (1, 1, 32 * 1024 * 1024 + 1),
    ] {
        assert!(CollectionLimits::new(limits.0, limits.1, limits.2).is_err());
    }
    for ceiling in [
        vec![],
        vec!["list".into(), "list".into()],
        vec!["bad\nname".into()],
    ] {
        assert!(
            RequiredCollectionOperation::new(
                "read".into(),
                json!({"repository":"one"}),
                ceiling,
                Arc::new(Pages)
            )
            .is_err()
        );
    }
    assert!(CollectionCall::new("list".into(), json!("text is not an argument object")).is_err());
    assert!(CollectionCall::new("list".into(), json!({"large":"x".repeat(32769)})).is_err());
    assert!(
        RequiredServiceCollection::new(
            "source".into(),
            "f".into(),
            "p".into(),
            "a".into(),
            vec![],
            CollectionLimits::new(1, 1, 1).unwrap()
        )
        .is_err()
    );
}
#[test]
fn a_fresh_cursor_uses_exact_schema_pages_and_rejects_foreign_resource() {
    let requirement = requirement("read-one", "one");
    let installed = selected();
    let target = || CollectionTarget {
        source_id: "source",
        requirement_id: &requirement.id,
        resource: &requirement.resource,
        operations: &requirement.operations,
        installed: &installed,
    };
    assert!(requirement.program.accepts_target(&target()));
    let mut first = requirement.program.start(&target());
    let mut second = requirement.program.start(&target());
    let CollectionStep::Call(call) = first.begin() else {
        panic!("first exact page");
    };
    assert_eq!(call.arguments, json!({"repository":"one","page":1}));
    let native = unqualified_schema_report();
    let result = ToolResult::new(
        "schema-only".into(),
        json!({"repository":"one","page":1,"next":2}).to_string(),
        false,
    );
    let CollectionStep::Call(next) = first.observe(&schema_observation(
        target(),
        &call.arguments,
        &result,
        &native,
    )) else {
        panic!("actual schema next page");
    };
    assert_eq!(next.arguments, json!({"repository":"one","page":2}));
    let CollectionStep::Call(reset) = second.begin() else {
        panic!("fresh attempt starts page one");
    };
    assert_eq!(reset.arguments, call.arguments);
    let wrong = ToolResult::new(
        "schema-only".into(),
        json!({"repository":"two","page":2,"next":null}).to_string(),
        false,
    );
    assert!(matches!(
        first.observe(&schema_observation(
            target(),
            &next.arguments,
            &wrong,
            &native
        )),
        CollectionStep::Finish(ResourceAssessment::NotVerified)
    ));
    let malformed = ToolResult::new(
        "schema-only".into(),
        "successful but no resource schema".into(),
        false,
    );
    assert!(matches!(
        first.observe(&schema_observation(
            target(),
            &next.arguments,
            &malformed,
            &native
        )),
        CollectionStep::Finish(ResourceAssessment::Unknown)
    ));
    let exact = ToolResult::new(
        "schema-only".into(),
        json!({"repository":"one","page":2,"next":null}).to_string(),
        false,
    );
    assert!(matches!(
        first.observe(&schema_observation(
            target(),
            &next.arguments,
            &exact,
            &native
        )),
        CollectionStep::Finish(ResourceAssessment::Verified)
    ));
    // This verifies only the schema program. All native fields remain absent;
    // only the Ready engine can supply such a body as runtime evidence.
    assert!(!native.native_terminal_committed && native.input_id.is_none());
}
#[test]
fn schema_program_checks_selected_native_target_and_coverage_has_no_ready_variant() {
    let r = requirement("read", "one");
    let mut installed = selected();
    installed.name = "foreign".into();
    assert!(!r.program.accepts_target(&CollectionTarget {
        source_id: "source",
        requirement_id: &r.id,
        resource: &r.resource,
        operations: &r.operations,
        installed: &installed
    }));
    assert_eq!(
        serde_json::to_value(ReusableSourceCoverage::UnqualifiedCredentialContinuity).unwrap(),
        json!("unqualified_credential_continuity")
    );
}
