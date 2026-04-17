use crate::Result;
use crate::protocol::models::{McpToolConfig, Tool};
use schemars::JsonSchema;
use schemars::schema::RootSchema;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

pub type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send + 'static>>;

type ToolHandler = Box<dyn Fn(Value) -> BoxFuture<Result<Value>> + Send + Sync>;

#[async_trait::async_trait]
pub trait ToolDispatcher: Send + Sync {
    async fn dispatch(&self, call: ToolCall) -> Result<ToolResult>;
    fn tool_definitions(&self) -> Vec<crate::protocol::models::Tool>;
    #[allow(clippy::result_large_err)]
    fn try_tool_definitions(&self) -> Result<Vec<crate::protocol::models::Tool>> {
        Ok(self.tool_definitions())
    }
}

#[derive(Clone, Debug)]
pub struct ToolDefinition {
    pub name: String,
    pub description: Option<String>,
    pub schema: RootSchema,
}

impl ToolDefinition {
    #[allow(clippy::result_large_err)]
    pub(crate) fn try_as_tool(&self) -> Result<Tool> {
        let parameters = serde_json::to_value(&self.schema)
            .map_err(|e| crate::Error::InvalidClientEvent(e.to_string()))?;
        Ok(Tool::Function {
            name: self.name.clone(),
            description: self.description.clone(),
            parameters,
        })
    }
}

#[derive(Clone, Debug)]
pub struct ToolCall {
    pub name: String,
    pub call_id: String,
    pub arguments: Value,
    pub response_id: Option<String>,
    pub item_id: Option<String>,
    pub output_index: Option<u32>,
}

#[derive(Clone, Debug)]
pub struct ToolResult {
    pub call_id: String,
    pub output: Value,
}

#[derive(Default)]
pub struct ToolRegistry {
    defs: Vec<ToolDefinition>,
    handlers: HashMap<String, ToolHandler>,
    mcp: Vec<McpToolConfig>,
}

impl ToolRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn definitions(&self) -> &[ToolDefinition] {
        &self.defs
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.defs.is_empty() && self.mcp.is_empty()
    }

    pub fn tool<TArgs, TResp, F, Fut>(&mut self, name: &str, handler: F)
    where
        TArgs: DeserializeOwned + JsonSchema + Send + 'static,
        TResp: Serialize + Send + 'static,
        F: Fn(TArgs) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<TResp>> + Send + 'static,
    {
        self.register_tool(name, None, handler);
    }

    pub fn tool_desc<TArgs, TResp, F, Fut>(
        &mut self,
        name: &str,
        description: impl Into<String>,
        handler: F,
    ) where
        TArgs: DeserializeOwned + JsonSchema + Send + 'static,
        TResp: Serialize + Send + 'static,
        F: Fn(TArgs) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<TResp>> + Send + 'static,
    {
        self.tool_with_description(name, description, handler);
    }

    pub fn tool_with_description<TArgs, TResp, F, Fut>(
        &mut self,
        name: &str,
        description: impl Into<String>,
        handler: F,
    ) where
        TArgs: DeserializeOwned + JsonSchema + Send + 'static,
        TResp: Serialize + Send + 'static,
        F: Fn(TArgs) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<TResp>> + Send + 'static,
    {
        self.register_tool(name, Some(description.into()), handler);
    }

    fn register_tool<TArgs, TResp, F, Fut>(
        &mut self,
        name: &str,
        description: Option<String>,
        handler: F,
    ) where
        TArgs: DeserializeOwned + JsonSchema + Send + 'static,
        TResp: Serialize + Send + 'static,
        F: Fn(TArgs) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<TResp>> + Send + 'static,
    {
        let schema = schemars::schema_for!(TArgs);
        let name = name.to_string();
        let entry = ToolDefinition {
            name: name.clone(),
            description,
            schema,
        };
        self.defs.push(entry);

        let user_handler = Arc::new(handler);
        let handler = move |value: Value| -> BoxFuture<Result<Value>> {
            let user_handler = Arc::clone(&user_handler);
            Box::pin(async move {
                let args: TArgs = serde_json::from_value(value)
                    .map_err(|e| crate::Error::InvalidClientEvent(e.to_string()))?;
                let resp = user_handler(args).await?;
                serde_json::to_value(resp)
                    .map_err(|e| crate::Error::InvalidClientEvent(e.to_string()))
            })
        };

        self.handlers.insert(name, Box::new(handler));
    }

    pub fn register<T: ToolSpec>(&mut self, tool: T) {
        let schema = schemars::schema_for!(T::Args);
        let entry = ToolDefinition {
            name: T::NAME.to_string(),
            description: T::DESCRIPTION.map(ToString::to_string),
            schema,
        };
        self.defs.push(entry);

        let tool = Arc::new(tool);
        let handler = move |value: Value| -> BoxFuture<Result<Value>> {
            let tool = Arc::clone(&tool);
            Box::pin(async move {
                let args: T::Args = serde_json::from_value(value)
                    .map_err(|e| crate::Error::InvalidClientEvent(e.to_string()))?;
                let resp = tool.call(args).await?;
                serde_json::to_value(resp)
                    .map_err(|e| crate::Error::InvalidClientEvent(e.to_string()))
            })
        };

        self.handlers.insert(T::NAME.to_string(), Box::new(handler));
    }

    /// Register an MCP tool configuration for the session.
    ///
    /// # Errors
    /// Returns an error if the MCP config is invalid.
    // Keep a single public error type for the SDK surface.
    #[allow(clippy::result_large_err)]
    pub fn mcp_tool(&mut self, config: McpToolConfig) -> Result<()> {
        config.validate()?;
        self.mcp.push(config);
        Ok(())
    }

    /// Convert all registered tools into protocol-level tool definitions.
    ///
    /// # Errors
    /// Returns an error if schema serialization fails.
    // Keep a single public error type for the SDK surface.
    #[allow(clippy::result_large_err)]
    pub fn try_as_tools(&self) -> Result<Vec<Tool>> {
        let mut tools = Vec::with_capacity(self.defs.len() + self.mcp.len());
        for def in &self.defs {
            tools.push(def.try_as_tool()?);
        }
        for mcp in &self.mcp {
            tools.push(Tool::Mcp(mcp.clone()));
        }
        Ok(tools)
    }
}

#[async_trait::async_trait]
impl ToolDispatcher for ToolRegistry {
    async fn dispatch(&self, call: ToolCall) -> Result<ToolResult> {
        let handler = self.handlers.get(&call.name).ok_or_else(|| {
            crate::Error::InvalidClientEvent(format!("unknown tool: {}", call.name))
        })?;
        let output = handler(call.arguments).await?;
        Ok(ToolResult {
            call_id: call.call_id,
            output,
        })
    }

    fn tool_definitions(&self) -> Vec<crate::protocol::models::Tool> {
        self.try_as_tools().unwrap_or_else(|err| {
            tracing::error!(error = %err, "failed to serialize tool definitions");
            Vec::new()
        })
    }

    fn try_tool_definitions(&self) -> Result<Vec<crate::protocol::models::Tool>> {
        self.try_as_tools()
    }
}

pub trait ToolSpec: Send + Sync + 'static {
    type Args: DeserializeOwned + JsonSchema + Send + 'static;
    type Output: Serialize + Send + 'static;
    const NAME: &'static str;
    const DESCRIPTION: Option<&'static str>;

    fn call(&self, args: Self::Args) -> BoxFuture<Result<Self::Output>>;
}

#[macro_export]
macro_rules! realtime_tool {
    (
        $(#[$meta:meta])*
        $name:ident :
        $args:ty => $resp:ty
        {
            name: $tool_name:expr,
            description: $desc:expr,
            $body:expr
        }
    ) => {
        $(#[$meta])*
        pub struct $name;

        impl $crate::ToolSpec for $name {
            type Args = $args;
            type Output = $resp;
            const NAME: &'static str = $tool_name;
            const DESCRIPTION: Option<&'static str> = Some($desc);

            fn call(&self, args: Self::Args) -> $crate::ToolFuture<$crate::Result<Self::Output>> {
                let fut = $body;
                Box::pin(fut(args))
            }
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn tool_without_description_omits_field() {
        let mut tools = ToolRegistry::new();
        tools.tool::<serde_json::Value, serde_json::Value, _, _>(
            "echo",
            |args| async move { Ok(args) },
        );

        let defs = tools.try_as_tools().unwrap();
        assert_eq!(defs.len(), 1);
        match &defs[0] {
            Tool::Function { description, .. } => {
                assert!(description.is_none());
            }
            other @ Tool::Mcp(_) => panic!("unexpected tool: {other:?}"),
        }
    }

    #[tokio::test]
    async fn tool_with_description_keeps_field() {
        let mut tools = ToolRegistry::new();
        tools.tool_desc::<serde_json::Value, serde_json::Value, _, _>(
            "echo",
            "desc",
            |args| async move { Ok(args) },
        );

        let defs = tools.try_as_tools().unwrap();
        assert_eq!(defs.len(), 1);
        match &defs[0] {
            Tool::Function { description, .. } => {
                assert_eq!(description.as_deref(), Some("desc"));
            }
            other @ Tool::Mcp(_) => panic!("unexpected tool: {other:?}"),
        }
    }
}
