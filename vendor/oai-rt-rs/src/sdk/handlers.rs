use crate::Result;
use crate::protocol::server_events::ServerEvent;
use std::future::Future;
use std::pin::Pin;

pub type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send + 'static>>;

pub type TextHandler = Box<dyn Fn(String) -> BoxFuture<Result<()>> + Send + Sync>;
pub type ToolCallHandler =
    Box<dyn Fn(super::ToolCall) -> BoxFuture<Result<super::ToolResult>> + Send + Sync>;
pub type RawEventHandler = Box<dyn Fn(ServerEvent) -> BoxFuture<Result<()>> + Send + Sync>;

#[derive(Default)]
pub struct EventHandlers {
    pub on_text: Option<TextHandler>,
    pub on_tool_call: Option<ToolCallHandler>,
    pub on_raw_event: Option<RawEventHandler>,
}

impl EventHandlers {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn on_text<F, Fut>(mut self, handler: F) -> Self
    where
        F: Fn(String) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<()>> + Send + 'static,
    {
        self.on_text = Some(Box::new(move |text| Box::pin(handler(text))));
        self
    }

    #[must_use]
    pub fn on_tool_call<F, Fut>(mut self, handler: F) -> Self
    where
        F: Fn(super::ToolCall) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<super::ToolResult>> + Send + 'static,
    {
        self.on_tool_call = Some(Box::new(move |call| Box::pin(handler(call))));
        self
    }

    #[must_use]
    pub fn on_raw_event<F, Fut>(mut self, handler: F) -> Self
    where
        F: Fn(ServerEvent) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<()>> + Send + 'static,
    {
        self.on_raw_event = Some(Box::new(move |evt| Box::pin(handler(evt))));
        self
    }
}
