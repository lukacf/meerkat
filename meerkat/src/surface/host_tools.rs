use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use meerkat_core::error::ToolError;
use meerkat_core::ops::ToolDispatchOutcome;
use meerkat_core::{AgentToolDispatcher, ToolCallView, ToolDef, ToolResult};

pub const HOST_TOOL_ACKNOWLEDGED: &str = "acknowledged";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostToolDispatchMode {
    Callback,
    FireAndForget,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostToolCallbackResult {
    pub content: String,
    pub is_error: bool,
}

impl HostToolCallbackResult {
    pub fn new(content: String, is_error: bool) -> Self {
        Self { content, is_error }
    }
}

enum HostToolMode<T> {
    Callback(T),
    FireAndForget,
}

struct HostToolEntry<T> {
    def: Arc<ToolDef>,
    mode: HostToolMode<T>,
}

pub struct HostToolRegistry<T> {
    entries: Vec<HostToolEntry<T>>,
}

impl<T> Default for HostToolRegistry<T> {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
        }
    }
}

impl<T> HostToolRegistry<T> {
    pub fn register_callback(
        &mut self,
        name: String,
        description: String,
        input_schema: Value,
        callback: T,
    ) {
        self.register(
            Arc::new(ToolDef {
                name,
                description,
                input_schema,
            }),
            HostToolMode::Callback(callback),
        );
    }

    pub fn register_fire_and_forget(
        &mut self,
        name: String,
        description: String,
        input_schema: Value,
    ) {
        self.register(
            Arc::new(ToolDef {
                name,
                description,
                input_schema,
            }),
            HostToolMode::FireAndForget,
        );
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }

    pub fn definitions(&self) -> Arc<[Arc<ToolDef>]> {
        self.entries
            .iter()
            .map(|entry| entry.def.clone())
            .collect::<Vec<_>>()
            .into()
    }

    pub fn dispatch_mode(&self, name: &str) -> Option<HostToolDispatchMode> {
        self.entries
            .iter()
            .find(|entry| entry.def.name == name)
            .map(|entry| match entry.mode {
                HostToolMode::Callback(_) => HostToolDispatchMode::Callback,
                HostToolMode::FireAndForget => HostToolDispatchMode::FireAndForget,
            })
    }

    fn register(&mut self, def: Arc<ToolDef>, mode: HostToolMode<T>) {
        self.entries.retain(|entry| entry.def.name != def.name);
        self.entries.push(HostToolEntry { def, mode });
    }
}

impl<T: Clone> HostToolRegistry<T> {
    pub fn callback(&self, name: &str) -> Option<T> {
        self.entries.iter().find_map(|entry| {
            if entry.def.name != name {
                return None;
            }

            match &entry.mode {
                HostToolMode::Callback(callback) => Some(callback.clone()),
                HostToolMode::FireAndForget => None,
            }
        })
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
pub trait HostToolBridge: Send + Sync {
    fn tools(&self) -> Arc<[Arc<ToolDef>]>;

    fn dispatch_mode(&self, name: &str) -> Option<HostToolDispatchMode>;

    async fn invoke_callback(
        &self,
        name: &str,
        args_json: &str,
    ) -> Result<HostToolCallbackResult, ToolError>;
}

pub struct HostToolDispatcher<B> {
    bridge: B,
}

impl<B> HostToolDispatcher<B> {
    pub fn new(bridge: B) -> Self {
        Self { bridge }
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl<B> AgentToolDispatcher for HostToolDispatcher<B>
where
    B: HostToolBridge,
{
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        self.bridge.tools()
    }

    async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
        match self.bridge.dispatch_mode(call.name) {
            Some(HostToolDispatchMode::FireAndForget) => Ok(ToolResult::new(
                call.id.to_string(),
                HOST_TOOL_ACKNOWLEDGED.to_string(),
                false,
            )
            .into()),
            Some(HostToolDispatchMode::Callback) => {
                let result = self
                    .bridge
                    .invoke_callback(call.name, call.args.get())
                    .await?;
                Ok(ToolResult::new(call.id.to_string(), result.content, result.is_error).into())
            }
            None => Err(ToolError::not_found(call.name)),
        }
    }
}

#[cfg(test)]
#[allow(clippy::panic)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::Mutex;

    use serde_json::json;
    use serde_json::value::RawValue;

    type CallbackFuture =
        Pin<Box<dyn Future<Output = Result<HostToolCallbackResult, ToolError>> + Send>>;
    type TestCallback = Arc<dyn Fn(String) -> CallbackFuture + Send + Sync>;

    struct TestBridge {
        registry: HostToolRegistry<TestCallback>,
    }

    #[async_trait]
    impl HostToolBridge for TestBridge {
        fn tools(&self) -> Arc<[Arc<ToolDef>]> {
            self.registry.definitions()
        }

        fn dispatch_mode(&self, name: &str) -> Option<HostToolDispatchMode> {
            self.registry.dispatch_mode(name)
        }

        async fn invoke_callback(
            &self,
            name: &str,
            args_json: &str,
        ) -> Result<HostToolCallbackResult, ToolError> {
            let callback = self
                .registry
                .callback(name)
                .ok_or_else(|| ToolError::not_found(name))?;
            callback(args_json.to_string()).await
        }
    }

    #[tokio::test]
    async fn host_tool_contract_callback_tools_await_and_parse_results() {
        let seen_args = Arc::new(Mutex::new(Vec::new()));
        let seen_args_for_callback = seen_args.clone();
        let callback: TestCallback = Arc::new(move |args_json| {
            let seen_args = seen_args_for_callback.clone();
            Box::pin(async move {
                seen_args.lock().unwrap().push(args_json.clone());
                Ok(HostToolCallbackResult::new(
                    "browser callback ok".to_string(),
                    false,
                ))
            })
        });

        let mut registry = HostToolRegistry::default();
        registry.register_callback(
            "echo_browser".to_string(),
            "Echo browser payloads".to_string(),
            json!({
                "type": "object",
                "properties": {
                    "value": { "type": "string" }
                },
                "required": ["value"]
            }),
            callback,
        );

        let dispatcher = HostToolDispatcher::new(TestBridge { registry });
        let args = RawValue::from_string(r#"{"value":"browser"}"#.to_string()).unwrap();
        let call = ToolCallView {
            id: "tc_1",
            name: "echo_browser",
            args: &args,
        };

        let outcome = dispatcher.dispatch(call).await.unwrap();

        assert_eq!(dispatcher.tools().len(), 1);
        assert_eq!(outcome.result.tool_use_id, "tc_1");
        assert_eq!(outcome.result.text_content(), "browser callback ok");
        assert!(!outcome.result.is_error);
        assert!(outcome.async_ops.is_empty());
        assert_eq!(
            seen_args.lock().unwrap().as_slice(),
            &[r#"{"value":"browser"}"#.to_string()]
        );
    }

    #[tokio::test]
    async fn host_tool_contract_fire_and_forget_returns_acknowledged() {
        let mut registry: HostToolRegistry<TestCallback> = HostToolRegistry::default();
        registry.register_fire_and_forget(
            "request_human_approval".to_string(),
            "Escalate a risky action".to_string(),
            json!({
                "type": "object",
                "properties": {
                    "summary": { "type": "string" }
                },
                "required": ["summary"]
            }),
        );

        let dispatcher = HostToolDispatcher::new(TestBridge { registry });
        let args = RawValue::from_string(r#"{"summary":"Needs sign-off"}"#.to_string()).unwrap();
        let call = ToolCallView {
            id: "tc_2",
            name: "request_human_approval",
            args: &args,
        };

        let outcome = dispatcher.dispatch(call).await.unwrap();

        assert_eq!(outcome.result.tool_use_id, "tc_2");
        assert_eq!(outcome.result.text_content(), HOST_TOOL_ACKNOWLEDGED);
        assert!(!outcome.result.is_error);
        assert!(outcome.async_ops.is_empty());
    }

    #[tokio::test]
    async fn host_tool_contract_callback_errors_are_propagated() {
        let callback: TestCallback = Arc::new(|_args_json| {
            Box::pin(async {
                Err(ToolError::execution_failed(
                    "callback failed before producing a typed result".to_string(),
                ))
            })
        });

        let mut registry = HostToolRegistry::default();
        registry.register_callback(
            "broken_browser".to_string(),
            "Broken browser callback".to_string(),
            json!({ "type": "object" }),
            callback,
        );

        let dispatcher = HostToolDispatcher::new(TestBridge { registry });
        let args = RawValue::from_string("{}".to_string()).unwrap();
        let call = ToolCallView {
            id: "tc_3",
            name: "broken_browser",
            args: &args,
        };

        let err = match dispatcher.dispatch(call).await {
            Ok(_) => panic!("expected callback error"),
            Err(err) => err,
        };

        assert!(
            err.to_string()
                .contains("callback failed before producing a typed result"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn host_tool_contract_registration_replaces_existing_tool_by_name() {
        let mut registry: HostToolRegistry<()> = HostToolRegistry::default();
        registry.register_fire_and_forget(
            "request_human_approval".to_string(),
            "first".to_string(),
            json!({ "type": "object" }),
        );
        registry.register_fire_and_forget(
            "request_human_approval".to_string(),
            "second".to_string(),
            json!({ "type": "object" }),
        );

        let tools = registry.definitions();

        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].description, "second");
        assert_eq!(
            registry.dispatch_mode("request_human_approval"),
            Some(HostToolDispatchMode::FireAndForget)
        );
    }
}
