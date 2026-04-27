use async_trait::async_trait;
use meerkat::LlmClient;
use meerkat_client::types::LlmStream;
use meerkat_client::{LlmError, LlmRequest, TestClient};
use meerkat_core::types::Message;
use std::sync::Mutex;

#[derive(Default)]
pub struct CaptureClient {
    inner: TestClient,
    seen_tools: Mutex<Vec<String>>,
    seen_user_messages: Mutex<Vec<String>>,
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
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl LlmClient for CaptureClient {
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
        self.inner.stream(request)
    }

    fn provider(&self) -> &'static str {
        self.inner.provider()
    }

    async fn health_check(&self) -> Result<(), LlmError> {
        self.inner.health_check().await
    }
}
