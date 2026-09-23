use async_trait::async_trait;
use meerkat::LlmClient;
use meerkat_client::types::LlmStream;
use meerkat_client::{LlmDoneOutcome, LlmError, LlmEvent, LlmRequest, TestClient};
use meerkat_core::types::Message;
use std::sync::Mutex;

#[derive(Default)]
enum CatalogProbe {
    #[default]
    Start,
    Search { call_id: String },
    Load { call_id: String, names: Vec<String> },
    Done,
}

#[derive(Default)]
pub struct CaptureClient {
    inner: TestClient,
    seen_tools: Mutex<Vec<String>>,
    seen_user_messages: Mutex<Vec<String>>,
    catalog_probe: Mutex<CatalogProbe>,
}

impl CaptureClient {
    pub fn tool_names(&self) -> Vec<String> {
        self.seen_tools.lock().expect("capture lock").clone()
    }

    pub fn user_messages(&self) -> Vec<String> {
        self.seen_user_messages
            .lock()
            .expect("capture lock")
            .clone()
    }

    fn control_result(request: &LlmRequest, call_id: &str) -> serde_json::Value {
        let result = request.messages.iter().rev().find_map(|message| match message {
            Message::ToolResults { results, .. } => {
                results.iter().find(|result| result.tool_use_id == call_id)
            }
            _ => None,
        }).expect("catalog control tool must return a result");
        assert!(!result.is_error, "catalog control failed: {result:?}");
        serde_json::from_str(&result.text_content()).expect("catalog control JSON")
    }

    fn control_call(call_id: String, name: &str, args: serde_json::Value) -> LlmStream<'static> {
        Box::pin(futures::stream::iter([
            Ok(LlmEvent::ToolCallComplete {
                id: call_id,
                name: name.into(),
                args,
                meta: None,
            }),
            Ok(LlmEvent::Done {
                outcome: LlmDoneOutcome::Success {
                    stop_reason: meerkat_core::StopReason::ToolUse,
                },
            }),
        ]))
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl LlmClient for CaptureClient {
    fn project_replay_messages(
        &self,
        messages: &[meerkat_core::Message],
    ) -> Result<Vec<meerkat_core::Message>, meerkat_client::LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(&'a self, request: &'a LlmRequest) -> LlmStream<'a> {
        *self.seen_tools.lock().expect("capture lock") =
            request.tools.iter().map(|tool| tool.name.to_string()).collect();
        let seen_user_messages = request
            .messages
            .iter()
            .filter_map(|message| match message {
                Message::User(user) => Some(user.text_content()),
                _ => None,
            })
            .collect();
        *self.seen_user_messages.lock().expect("capture lock") = seen_user_messages;
        // Exercise the real discovery plane instead of assuming deferred tools
        // must already be inline in the first model request.
        let mut probe = self.catalog_probe.lock().expect("catalog probe lock");
        match &*probe {
            CatalogProbe::Start if request.tools.iter().any(|tool| tool.name == "tool_catalog_search") => {
                let call_id = format!("capture_search_{}", uuid::Uuid::new_v4());
                *probe = CatalogProbe::Search { call_id: call_id.clone() };
                return Self::control_call(call_id, "tool_catalog_search", serde_json::json!({"limit": 50}));
            }
            CatalogProbe::Search { call_id } => {
                let response = Self::control_result(request, call_id);
                assert_eq!(response["catalog_exact"], true);
                let names: Vec<String> = response["results"].as_array().expect("search results")
                    .iter().filter(|entry| entry["visibility_status"] == "deferred")
                    .map(|entry| entry["name"].as_str().expect("catalog name").to_string())
                    .collect();
                if !names.is_empty() {
                    let call_id = format!("capture_load_{}", uuid::Uuid::new_v4());
                    let args = serde_json::json!({"names": names});
                    *probe = CatalogProbe::Load { call_id: call_id.clone(), names };
                    return Self::control_call(call_id, "tool_catalog_load", args);
                }
                *probe = CatalogProbe::Done;
            }
            CatalogProbe::Load { call_id, names } => {
                let response = Self::control_result(request, call_id);
                for name in names {
                    assert!(
                        response["accepted_names"].as_array().is_some_and(|accepted| accepted.iter().any(|value| value == name))
                        || response["noop_names"].as_array().is_some_and(|accepted| accepted.iter().any(|value| value == name)),
                        "catalog did not load {name}: {response}",
                    );
                }
                *probe = CatalogProbe::Done;
            }
            CatalogProbe::Start | CatalogProbe::Done => {}
        }
        drop(probe);
        self.inner.stream(request)
    }

    fn provider(&self) -> meerkat_core::Provider {
        self.inner.provider()
    }

    async fn health_check(&self) -> Result<(), LlmError> {
        self.inner.health_check().await
    }
}
