use async_trait::async_trait;
use meerkat::LlmClient;
use meerkat_client::types::LlmStream;
use meerkat_client::{LlmError, LlmRequest, TestClient};
use std::sync::Mutex;

#[derive(Default)]
pub struct CaptureClient {
    inner: TestClient,
    seen_tools: Mutex<Vec<String>>,
}

impl CaptureClient {
    pub fn tool_names(&self) -> Vec<String> {
        self.seen_tools.lock().expect("capture lock").clone()
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl LlmClient for CaptureClient {
    fn stream<'a>(&'a self, request: &'a LlmRequest) -> LlmStream<'a> {
        *self.seen_tools.lock().expect("capture lock") =
            request.tools.iter().map(|tool| tool.name.clone()).collect();
        self.inner.stream(request)
    }

    fn provider(&self) -> &'static str {
        self.inner.provider()
    }

    async fn health_check(&self) -> Result<(), LlmError> {
        self.inner.health_check().await
    }
}
