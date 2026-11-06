//! The `request_tool_grant` tool of a grant-gated build.

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use super::*;
    use crate::RunId;
    use crate::agent::{AgentToolDispatcher, ToolDispatchContext};
    use crate::approval::decision::{DecisionId, DecisionKind, DecisionPhase, DecisionSubjectKey};
    use crate::error::ToolError;
    use crate::tool_grant::{
        GatedToolTable, REQUEST_TOOL_GRANT_TOOL, SessionToolGrants, ToolGrantAgentRequest,
        ToolGrantAgentRequestRefusal, ToolGrantCall, ToolGrantGate, ToolGrantId, ToolGrantRefusal,
        ToolGrantRequestReceipt, ToolGrantTarget,
    };
    use crate::types::{ToolCallView, ToolName, ToolNameSet};

    /// One request the seam was asked to hold.
    #[derive(Debug, Clone, PartialEq, Eq)]
    struct Asked {
        target: ToolGrantTarget,
        duration: Duration,
        reason: Option<String>,
        run_id: RunId,
        call_id: String,
    }

    /// A session's grants that take requests and record each one.
    #[derive(Default)]
    struct RecordingGrants {
        asked: Mutex<Vec<Asked>>,
    }

    impl RecordingGrants {
        fn asked(&self) -> Vec<Asked> {
            self.asked.lock().unwrap().clone()
        }
    }

    #[async_trait::async_trait]
    impl SessionToolGrants for RecordingGrants {
        async fn open_names(
            &self,
            _gated: &GatedToolTable,
            _run_id: Option<&RunId>,
        ) -> Result<ToolNameSet, ToolGrantRefusal> {
            Ok(ToolNameSet::new())
        }

        async fn admit_call(
            &self,
            _gated: &GatedToolTable,
            _call: ToolGrantCall<'_>,
        ) -> Result<(), ToolGrantRefusal> {
            Err(ToolGrantRefusal::Required)
        }

        async fn end_run(&self, _run_id: &RunId) -> Result<(), ToolGrantRefusal> {
            Ok(())
        }

        async fn request_grant(
            &self,
            request: ToolGrantAgentRequest<'_>,
        ) -> Result<ToolGrantRequestReceipt, ToolGrantAgentRequestRefusal> {
            self.asked.lock().unwrap().push(Asked {
                target: request.target.clone(),
                duration: request.duration,
                reason: request.reason.map(str::to_owned),
                run_id: request.run_id.clone(),
                call_id: request.call_id.to_owned(),
            });
            let decision_id = DecisionId::derive(
                &DecisionKind::new("tool_grant.issue").unwrap(),
                &DecisionSubjectKey::new(format!(
                    "run:{}:call:{}",
                    request.run_id, request.call_id
                ))
                .unwrap(),
            );
            Ok(ToolGrantRequestReceipt {
                grant_id: ToolGrantId::approved(&decision_id),
                decision_id,
                phase: DecisionPhase::Open,
                deadline_at_ms: 1_800_000_000_000,
            })
        }
    }

    fn shell_names() -> BTreeSet<ToolName> {
        ["shell", "shell_job_status"]
            .into_iter()
            .map(ToolName::from)
            .collect()
    }

    /// The `request_tool_grant` tool of a build that gates the shell family
    /// and the tool `deploy`, over `grants`.
    fn dispatcher(grants: &Arc<RecordingGrants>) -> ToolGrantRequestDispatcher {
        let table = GatedToolTable::new(
            BTreeMap::from([("shell".to_string(), shell_names())]),
            ["deploy"].into_iter().map(ToolName::from).collect(),
        );
        ToolGrantRequestDispatcher::new(ToolGrantGate::new(
            Arc::clone(grants) as Arc<dyn SessionToolGrants>,
            table,
        ))
    }

    fn in_run(run_id: &RunId) -> ToolDispatchContext {
        let mut context = ToolDispatchContext::default();
        context.bind_run_id(run_id.clone());
        context
    }

    async fn call(
        dispatcher: &ToolGrantRequestDispatcher,
        context: &ToolDispatchContext,
        call_id: &str,
        args: serde_json::Value,
    ) -> Result<crate::ops::ToolDispatchOutcome, ToolError> {
        let args = serde_json::value::RawValue::from_string(args.to_string()).unwrap();
        dispatcher
            .dispatch_with_context(
                ToolCallView {
                    id: call_id,
                    name: REQUEST_TOOL_GRANT_TOOL,
                    args: &args,
                },
                context,
            )
            .await
    }

    /// The agent names a gated target, a duration and a reason: never the
    /// subject (its own session) nor the approver (the host's policy). The
    /// seam is asked with the call's run and id, so a replayed call answers
    /// the same request.
    #[tokio::test]
    async fn a_request_from_the_agent_tool_names_no_approver_and_no_subject() {
        let grants = Arc::new(RecordingGrants::default());
        let dispatcher = dispatcher(&grants);

        let tools = dispatcher.tools();
        assert_eq!(tools.len(), 1);
        let tool = &tools[0];
        assert_eq!(tool.name.as_str(), REQUEST_TOOL_GRANT_TOOL);
        let schema = &tool.input_schema;
        let properties: BTreeSet<&str> = schema["properties"]
            .as_object()
            .expect("an object schema")
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            properties,
            BTreeSet::from(["tool", "family", "duration_ms", "reason"]),
            "{schema}"
        );
        assert_eq!(schema["additionalProperties"], serde_json::json!(false));
        assert_eq!(schema["required"], serde_json::json!(["duration_ms"]));

        let run_id = RunId::new();
        let context = in_run(&run_id);
        let outcome = call(
            &dispatcher,
            &context,
            "call-1",
            serde_json::json!({"family": "shell", "duration_ms": 600_000, "reason": "rotate the logs"}),
        )
        .await
        .expect("the request is held");
        assert!(!outcome.result.is_error);
        let answer: serde_json::Value =
            serde_json::from_str(&outcome.result.text_content()).expect("a JSON answer");
        assert_eq!(answer["status"], "requested", "{answer}");
        let decision_id = answer["request_id"].as_str().expect("the request's id");
        assert_eq!(
            answer["grant_id"].as_str(),
            Some(
                ToolGrantId::approved(&DecisionId::new(decision_id).unwrap())
                    .to_string()
                    .as_str()
            )
        );
        assert_eq!(
            grants.asked(),
            vec![Asked {
                target: ToolGrantTarget::Family {
                    family: "shell".to_string(),
                    names: shell_names(),
                },
                duration: Duration::from_mins(10),
                reason: Some("rotate the logs".to_string()),
                run_id: run_id.clone(),
                call_id: "call-1".to_string(),
            }]
        );

        // The subject and the approver are not the agent's to name, a
        // target the build does not gate is refused, and so is a request
        // naming both or neither of a tool and a family.
        for refused in [
            serde_json::json!({"family": "shell", "duration_ms": 1_000, "approver_id": "me"}),
            serde_json::json!({"family": "shell", "duration_ms": 1_000, "subject": {"kind": "session", "session_id": "x"}}),
            serde_json::json!({"tool": "web_search", "duration_ms": 1_000}),
            serde_json::json!({"family": "tasks", "duration_ms": 1_000}),
            serde_json::json!({"tool": "deploy", "family": "shell", "duration_ms": 1_000}),
            serde_json::json!({"duration_ms": 1_000}),
            serde_json::json!({"tool": "deploy"}),
        ] {
            let error = call(&dispatcher, &context, "call-2", refused.clone())
                .await
                .expect_err("refused");
            assert!(
                matches!(error, ToolError::InvalidArguments { .. }),
                "{refused}: {error:?}"
            );
        }
        // A gated tool name, by name or through its family, is a target.
        call(
            &dispatcher,
            &context,
            "call-3",
            serde_json::json!({"tool": "shell_job_status", "duration_ms": 1_000}),
        )
        .await
        .expect("a name of a gated family");
        call(
            &dispatcher,
            &context,
            "call-4",
            serde_json::json!({"tool": "deploy", "duration_ms": 1_000}),
        )
        .await
        .expect("a gated name");
        assert_eq!(grants.asked().len(), 3, "no refused call reached the seam");

        // A call outside any run is not one a request can name.
        let error = call(
            &dispatcher,
            &ToolDispatchContext::default(),
            "call-5",
            serde_json::json!({"tool": "deploy", "duration_ms": 1_000}),
        )
        .await
        .expect_err("a request is asked from a run");
        assert!(
            !matches!(error, ToolError::InvalidArguments { .. }),
            "{error:?}"
        );
        assert_eq!(grants.asked().len(), 3);
    }

    /// The tool answers the seam's refusal as a tool error: a host that takes
    /// no requests holds nothing.
    #[tokio::test]
    async fn a_request_the_host_does_not_take_is_a_tool_error() {
        struct NoRequests;

        #[async_trait::async_trait]
        impl SessionToolGrants for NoRequests {
            async fn open_names(
                &self,
                _gated: &GatedToolTable,
                _run_id: Option<&RunId>,
            ) -> Result<ToolNameSet, ToolGrantRefusal> {
                Ok(ToolNameSet::new())
            }

            async fn admit_call(
                &self,
                _gated: &GatedToolTable,
                _call: ToolGrantCall<'_>,
            ) -> Result<(), ToolGrantRefusal> {
                Err(ToolGrantRefusal::Required)
            }

            async fn end_run(&self, _run_id: &RunId) -> Result<(), ToolGrantRefusal> {
                Ok(())
            }
        }

        let grants: Arc<dyn SessionToolGrants> = Arc::new(NoRequests);
        assert_eq!(
            grants
                .request_grant(ToolGrantAgentRequest {
                    target: &ToolGrantTarget::Tool {
                        name: ToolName::from("deploy"),
                    },
                    duration: Duration::from_secs(1),
                    reason: None,
                    run_id: &RunId::new(),
                    call_id: "call-0",
                })
                .await,
            Err(ToolGrantAgentRequestRefusal::NotTaken),
            "the default takes no requests"
        );
        let gate = ToolGrantGate::new(
            grants,
            GatedToolTable::new(
                BTreeMap::new(),
                ["deploy"].into_iter().map(ToolName::from).collect(),
            ),
        );
        let dispatcher = ToolGrantRequestDispatcher::new(gate);
        let run_id = RunId::new();
        let error = call(
            &dispatcher,
            &in_run(&run_id),
            "call-1",
            serde_json::json!({"tool": "deploy", "duration_ms": 1_000}),
        )
        .await
        .expect_err("nothing is held");
        assert!(
            !matches!(error, ToolError::InvalidArguments { .. }),
            "{error:?}"
        );
    }
}
