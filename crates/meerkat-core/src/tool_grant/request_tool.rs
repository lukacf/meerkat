//! The `request_tool_grant` tool of a grant-gated build.
//!
//! The agent asks for a grant of one of the names or families its build
//! gates, which a person approves: it names the target, how long it needs it
//! and why, never the subject (its own session) nor the approver (the host's
//! policy). The tool holds nothing itself. The session's grant seam asks its
//! host to hold a durable decision ([`super::SessionToolGrants::request_grant`]),
//! keyed by the call's run and id so a replayed call answers the request it
//! held, and the call returns at once with the pending request. Nothing opens
//! until a person approves it and the host issues the grant; the session is
//! told when the request is decided.
//!
//! The factory mounts it beside a gated build's tools when the session's host
//! takes requests ([`super::ToolGrantGate::takes_requests`]). The tool itself
//! is not gated: every other constraint (the static tool policy, governed
//! authority, review) applies to it as to any tool.

use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;

use super::{
    REQUEST_TOOL_GRANT_TOOL, ToolGrantAgentRequest, ToolGrantAgentRequestRefusal, ToolGrantGate,
    ToolGrantRequestReceipt, format_unix_ms,
};
use crate::ToolCatalogCapabilities;
use crate::agent::{AgentToolDispatcher, ToolDispatchContext};
use crate::approval::decision::DecisionPhase;
use crate::error::ToolError;
use crate::ops::ToolDispatchOutcome;
use crate::types::{ToolCallView, ToolDef, ToolProvenance, ToolResult, ToolSourceKind};

/// The source id the request tool's provenance names.
const REQUEST_TOOL_SOURCE_ID: &str = "tool_grants";

/// What the agent names: exactly one of a gated tool name and a gated family,
/// how long the grant lasts once issued, and why.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RequestToolGrantArgs {
    #[serde(default)]
    tool: Option<String>,
    #[serde(default)]
    family: Option<String>,
    duration_ms: u64,
    #[serde(default)]
    reason: Option<String>,
}

/// The `request_tool_grant` tool over a build's grant gate; see the module
/// documentation.
pub struct ToolGrantRequestDispatcher {
    gate: ToolGrantGate,
    tools: Arc<[Arc<ToolDef>]>,
}

impl ToolGrantRequestDispatcher {
    /// The request tool of the build `gate` belongs to. Its description
    /// names what the build holds back.
    #[must_use]
    pub fn new(gate: ToolGrantGate) -> Self {
        let tool = Arc::new(request_tool_def(&gate));
        Self {
            gate,
            tools: Arc::from([tool]),
        }
    }

    async fn request(
        &self,
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
    ) -> Result<ToolDispatchOutcome, ToolError> {
        if call.name != REQUEST_TOOL_GRANT_TOOL {
            return Err(ToolError::not_found(call.name));
        }
        let args: RequestToolGrantArgs = call
            .parse_args()
            .map_err(|error| ToolError::invalid_arguments(call.name, error.to_string()))?;
        let Some(target) = self
            .gate
            .requested_target(args.tool.as_deref(), args.family.as_deref())
        else {
            return Err(ToolError::invalid_arguments(
                call.name,
                "name exactly one of `tool` and `family`: a tool name or a tool family this \
                 session holds back until it is granted",
            ));
        };
        if args.duration_ms == 0 {
            return Err(ToolError::invalid_arguments(
                call.name,
                "`duration_ms` must be at least 1",
            ));
        }
        let Some(run_id) = context.run_id() else {
            return Err(ToolError::execution_failed(
                "a tool grant is requested from a run of the session",
            ));
        };
        let receipt = self
            .gate
            .request_grant(ToolGrantAgentRequest {
                target: &target,
                duration: Duration::from_millis(args.duration_ms),
                reason: args.reason.as_deref(),
                run_id,
                call_id: call.id,
            })
            .await
            .map_err(|refusal| match refusal {
                ToolGrantAgentRequestRefusal::Invalid(reason) => {
                    ToolError::invalid_arguments(call.name, reason)
                }
                other => ToolError::execution_failed(other.to_string()),
            })?;
        Ok(ToolDispatchOutcome::from(ToolResult::new(
            call.id.to_owned(),
            answer(&receipt).to_string(),
            false,
        )))
    }
}

/// What the agent reads back: the request as it stands now.
fn answer(receipt: &ToolGrantRequestReceipt) -> serde_json::Value {
    let status = match receipt.phase {
        DecisionPhase::Open | DecisionPhase::Approved => "requested",
        DecisionPhase::Executed => "granted",
        _ => "declined",
    };
    serde_json::json!({
        "status": status,
        "request_id": receipt.decision_id.as_str(),
        "grant_id": receipt.grant_id.to_string(),
        "decide_by": format_unix_ms(receipt.deadline_at_ms),
        "note": "A person decides this request. Nothing opens until the grant is issued; \
                 the session is told when the request is decided.",
    })
}

/// The tool's definition: what it takes, and what the build holds back.
fn request_tool_def(gate: &ToolGrantGate) -> ToolDef {
    let table = gate.table();
    let mut held_back: Vec<String> = table
        .families()
        .iter()
        .map(|(family, names)| {
            let names: Vec<&str> = names.iter().map(|name| name.as_str()).collect();
            format!("family `{family}` ({})", names.join(", "))
        })
        .collect();
    held_back.extend(table.tools().iter().map(|name| format!("tool `{name}`")));
    let description = format!(
        "Ask for access to a tool or tool family this session holds back until a person \
         grants it. Name exactly one gated `tool` or `family`, how long you need it \
         (`duration_ms`) and why (`reason`). A person decides: the call returns at once \
         with the pending request, nothing opens until the grant is issued, and the session \
         is told when the request is decided. Held back: {}.",
        held_back.join("; ")
    );
    ToolDef::new(
        REQUEST_TOOL_GRANT_TOOL,
        description,
        serde_json::json!({
            "type": "object",
            "properties": {
                "tool": {
                    "type": "string",
                    "description": "One tool name this session holds back."
                },
                "family": {
                    "type": "string",
                    "description": "One tool family this session holds back."
                },
                "duration_ms": {
                    "type": "integer",
                    "minimum": 1,
                    "description": "How long the grant lasts once it is issued, in milliseconds."
                },
                "reason": {
                    "type": "string",
                    "description": "Why you need it, for the person who decides."
                }
            },
            "required": ["duration_ms"],
            "additionalProperties": false
        }),
    )
    .with_provenance(ToolProvenance {
        kind: ToolSourceKind::Builtin,
        source_id: REQUEST_TOOL_SOURCE_ID.into(),
        artifact: None,
    })
}

#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
impl AgentToolDispatcher for ToolGrantRequestDispatcher {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        Arc::clone(&self.tools)
    }

    /// Its catalog is exactly its one tool, so a composite that holds it
    /// stays exact.
    fn tool_catalog_capabilities(&self) -> ToolCatalogCapabilities {
        ToolCatalogCapabilities {
            exact_catalog: true,
            may_require_catalog_control_plane: false,
        }
    }

    async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
        self.request(call, &ToolDispatchContext::default()).await
    }

    async fn dispatch_with_context(
        &self,
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
    ) -> Result<ToolDispatchOutcome, ToolError> {
        self.request(call, context).await
    }
}

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
