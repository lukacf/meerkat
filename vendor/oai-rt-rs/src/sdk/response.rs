use crate::Result;
use crate::protocol::models::{
    ContentPart, InputItem, OutputModalities, ResponseConfig, Role, ToolChoice,
};
use crate::protocol::models::{MaxTokens, Metadata, Temperature, Voice};

use super::Session;
use super::ToolRegistry;

pub struct ResponseBuilder {
    config: ResponseConfig,
}

impl ResponseBuilder {
    #[must_use]
    pub fn new() -> Self {
        Self {
            config: ResponseConfig::default(),
        }
    }

    #[must_use]
    pub const fn output_text(mut self) -> Self {
        self.config.output_modalities = Some(OutputModalities::Text);
        self
    }

    #[must_use]
    pub const fn output_audio(mut self) -> Self {
        self.config.output_modalities = Some(OutputModalities::Audio);
        self
    }

    #[must_use]
    pub fn instructions(mut self, instructions: impl Into<String>) -> Self {
        self.config.instructions = Some(instructions.into());
        self
    }

    #[must_use]
    pub const fn temperature(mut self, temperature: Temperature) -> Self {
        self.config.temperature = Some(temperature);
        self
    }

    #[must_use]
    pub const fn max_output_tokens(mut self, max: MaxTokens) -> Self {
        self.config.max_output_tokens = Some(max);
        self
    }

    #[must_use]
    pub fn voice(mut self, voice: Voice) -> Self {
        self.config.voice = Some(voice);
        self
    }

    #[must_use]
    pub fn metadata(mut self, metadata: Metadata) -> Self {
        self.config.metadata = Some(metadata);
        self
    }

    #[must_use]
    pub fn tool_choice(mut self, choice: ToolChoice) -> Self {
        self.config.tool_choice = Some(choice);
        self
    }

    /// # Errors
    /// Returns an error if tool schema serialization fails.
    // Keep a single public error type for the SDK surface.
    #[allow(clippy::result_large_err)]
    pub fn tools(mut self, registry: &ToolRegistry) -> Result<Self> {
        if !registry.is_empty() {
            self.config.tools = Some(registry.try_as_tools()?);
        }
        Ok(self)
    }

    #[must_use]
    pub fn input_text(mut self, text: impl Into<String>) -> Self {
        let item = InputItem::Message {
            id: None,
            role: Role::User,
            content: vec![ContentPart::InputText { text: text.into() }],
        };
        self.push_input(item);
        self
    }

    #[must_use]
    pub fn input_item(mut self, item: InputItem) -> Self {
        self.push_input(item);
        self
    }

    #[must_use]
    pub fn build(self) -> ResponseConfig {
        self.config
    }

    /// Send the response using an active session.
    ///
    /// # Errors
    /// Returns an error if the SDK is not fully initialized or the send fails.
    pub async fn send(self, session: &Session) -> Result<()> {
        session.send_response(self.config).await
    }

    fn push_input(&mut self, item: InputItem) {
        self.config.input.get_or_insert_with(Vec::new).push(item);
    }
}

impl Default for ResponseBuilder {
    fn default() -> Self {
        Self::new()
    }
}
