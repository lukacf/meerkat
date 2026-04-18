use crate::protocol::models::{
    AudioConfig, AudioFormat, InputAudioConfig, InputAudioTranscription, MaxTokens, NoiseReduction,
    OutputAudioConfig, OutputModalities, SessionConfig, SessionKind, Temperature, ToolChoice,
    TurnDetection,
};
use crate::{Error, Result};
use std::sync::Arc;

use super::EventHandlers;
use super::session::SessionConfigSnapshot;
use super::tools::{ToolDispatcher, ToolRegistry};

pub struct Realtime;

impl Realtime {
    #[must_use]
    pub fn builder() -> RealtimeBuilder {
        RealtimeBuilder::new()
    }

    /// Connect via WebSocket with defaults.
    ///
    /// # Errors
    /// Returns an error if the connection fails.
    pub async fn connect_ws(api_key: &str) -> Result<super::Session> {
        RealtimeBuilder::new().api_key(api_key).connect_ws().await
    }
}

pub struct RealtimeBuilder {
    api_key: Option<String>,
    model: Option<String>,
    call_id: Option<String>,
    voice: Option<String>,
    session_kind: SessionKind,
    output_modalities: Option<OutputModalities>,
    instructions: Option<String>,
    tool_choice: Option<ToolChoice>,
    temperature: Option<Temperature>,
    max_output_tokens: Option<MaxTokens>,
    audio: Option<AudioConfig>,
    auto_barge_in: bool,
    auto_tool_response: bool,
    send_initial_session_update: bool,
    handlers: EventHandlers,
    tools: ToolRegistry,
    dispatcher: Option<Arc<dyn ToolDispatcher>>,
}

impl RealtimeBuilder {
    #[must_use]
    pub fn new() -> Self {
        Self {
            api_key: None,
            model: None,
            call_id: None,
            voice: None,
            session_kind: SessionKind::Realtime,
            output_modalities: None,
            instructions: None,
            tool_choice: None,
            temperature: None,
            max_output_tokens: None,
            audio: None,
            auto_barge_in: false,
            auto_tool_response: true,
            send_initial_session_update: true,
            handlers: EventHandlers::new(),
            tools: ToolRegistry::new(),
            dispatcher: None,
        }
    }

    #[must_use]
    pub fn api_key(mut self, key: impl Into<String>) -> Self {
        self.api_key = Some(key.into());
        self
    }

    #[must_use]
    pub fn model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }

    #[must_use]
    pub fn call_id(mut self, call_id: impl Into<String>) -> Self {
        self.call_id = Some(call_id.into());
        self
    }

    #[must_use]
    pub fn voice(mut self, voice: impl Into<String>) -> Self {
        let voice = voice.into();
        self.voice = Some(voice.clone());
        let output_voice = Some(crate::protocol::models::Voice::from(voice));
        match self.audio.as_mut() {
            Some(audio) => {
                let output = audio.output.get_or_insert_with(OutputAudioConfig::default);
                output.voice = output_voice;
            }
            None => {
                self.audio = Some(AudioConfig {
                    input: None,
                    output: Some(OutputAudioConfig {
                        format: None,
                        voice: output_voice,
                        speed: None,
                    }),
                });
            }
        }
        self
    }

    #[must_use]
    pub const fn session_kind(mut self, kind: SessionKind) -> Self {
        self.session_kind = kind;
        self
    }

    #[must_use]
    pub const fn transcription_session(mut self) -> Self {
        self.session_kind = SessionKind::Transcription;
        self
    }

    #[must_use]
    pub fn instructions(mut self, instructions: impl Into<String>) -> Self {
        self.instructions = Some(instructions.into());
        self
    }

    #[must_use]
    pub fn tool_choice(mut self, choice: ToolChoice) -> Self {
        self.tool_choice = Some(choice);
        self
    }

    #[must_use]
    pub const fn temperature(mut self, temperature: Temperature) -> Self {
        self.temperature = Some(temperature);
        self
    }

    #[must_use]
    pub const fn max_output_tokens(mut self, max_output_tokens: MaxTokens) -> Self {
        self.max_output_tokens = Some(max_output_tokens);
        self
    }

    #[must_use]
    pub const fn auto_barge_in(mut self, enabled: bool) -> Self {
        self.auto_barge_in = enabled;
        self
    }

    #[must_use]
    pub const fn auto_tool_response(mut self, enabled: bool) -> Self {
        self.auto_tool_response = enabled;
        self
    }

    #[must_use]
    pub const fn manual_sideband_control(mut self) -> Self {
        self.auto_barge_in = false;
        self.auto_tool_response = false;
        self.send_initial_session_update = false;
        self
    }

    #[must_use]
    pub fn tool_dispatcher(mut self, dispatcher: Arc<dyn ToolDispatcher>) -> Self {
        self.dispatcher = Some(dispatcher);
        self
    }

    #[must_use]
    pub fn voice_session(self) -> VoiceSessionBuilder {
        VoiceSessionBuilder::new(self)
    }

    #[must_use]
    pub const fn output_audio(mut self) -> Self {
        self.output_modalities = Some(OutputModalities::Audio);
        self
    }

    #[must_use]
    pub const fn output_text(mut self) -> Self {
        self.output_modalities = Some(OutputModalities::Text);
        self
    }

    #[must_use]
    pub fn tools(mut self, tools: ToolRegistry) -> Self {
        self.tools = tools;
        self
    }

    #[must_use]
    pub fn tool<TArgs, TResp, F, Fut>(mut self, name: &str, handler: F) -> Self
    where
        TArgs: schemars::JsonSchema + serde::de::DeserializeOwned + Send + 'static,
        TResp: serde::Serialize + Send + 'static,
        F: Fn(TArgs) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = Result<TResp>> + Send + 'static,
    {
        self.tools.tool(name, handler);
        self
    }

    #[must_use]
    pub fn tool_desc<TArgs, TResp, F, Fut>(
        mut self,
        name: &str,
        description: impl Into<String>,
        handler: F,
    ) -> Self
    where
        TArgs: schemars::JsonSchema + serde::de::DeserializeOwned + Send + 'static,
        TResp: serde::Serialize + Send + 'static,
        F: Fn(TArgs) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = Result<TResp>> + Send + 'static,
    {
        self.tools.tool_desc(name, description, handler);
        self
    }

    #[must_use]
    pub fn tool_with_description<TArgs, TResp, F, Fut>(
        mut self,
        name: &str,
        description: impl Into<String>,
        handler: F,
    ) -> Self
    where
        TArgs: schemars::JsonSchema + serde::de::DeserializeOwned + Send + 'static,
        TResp: serde::Serialize + Send + 'static,
        F: Fn(TArgs) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = Result<TResp>> + Send + 'static,
    {
        self.tools.tool_with_description(name, description, handler);
        self
    }

    /// # Errors
    /// Returns an error if the MCP tool configuration is invalid.
    // Keep a single public error type for the SDK surface.
    #[allow(clippy::result_large_err)]
    pub fn mcp_tool(mut self, config: crate::protocol::models::McpToolConfig) -> Result<Self> {
        self.tools.mcp_tool(config)?;
        Ok(self)
    }

    #[must_use]
    pub fn handlers(mut self, handlers: EventHandlers) -> Self {
        self.handlers = handlers;
        self
    }

    #[must_use]
    pub fn on_text<F, Fut>(mut self, handler: F) -> Self
    where
        F: Fn(String) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = Result<()>> + Send + 'static,
    {
        self.handlers = self.handlers.on_text(handler);
        self
    }

    #[must_use]
    pub fn on_tool_call<F, Fut>(mut self, handler: F) -> Self
    where
        F: Fn(super::ToolCall) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = Result<super::ToolResult>> + Send + 'static,
    {
        self.handlers = self.handlers.on_tool_call(handler);
        self
    }

    #[must_use]
    pub fn on_raw_event<F, Fut>(mut self, handler: F) -> Self
    where
        F: Fn(crate::protocol::server_events::ServerEvent) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = Result<()>> + Send + 'static,
    {
        self.handlers = self.handlers.on_raw_event(handler);
        self
    }

    #[allow(clippy::result_large_err)]
    fn build(self) -> Result<SessionConfigSnapshot> {
        let api_key = self
            .api_key
            .ok_or_else(|| Error::InvalidClientEvent("api_key required".to_string()))?;
        let model = self.model.clone();
        let output_modalities = self.output_modalities.unwrap_or(OutputModalities::Audio);
        let model_name = self
            .model
            .unwrap_or_else(|| crate::protocol::models::DEFAULT_MODEL.to_string());

        let mut session = SessionConfig::new(self.session_kind, model_name, output_modalities);
        session.instructions = self.instructions;
        session.tool_choice = self.tool_choice;
        session.temperature = self.temperature;
        session.max_output_tokens = self.max_output_tokens;
        if let Some(audio) = self.audio {
            session.audio = Some(audio);
        }

        let dispatcher = if let Some(d) = self.dispatcher {
            if session.tools.is_none() {
                let defs = d.try_tool_definitions()?;
                if !defs.is_empty() {
                    session.tools = Some(defs);
                }
            }
            d
        } else {
            if !self.tools.is_empty() {
                session.tools = Some(self.tools.try_as_tools()?);
            }
            Arc::new(self.tools)
        };

        Ok(SessionConfigSnapshot {
            api_key,
            model,
            call_id: self.call_id,
            session,
            handlers: self.handlers,
            dispatcher,
            auto_barge_in: self.auto_barge_in,
            auto_tool_response: self.auto_tool_response,
            send_initial_session_update: self.send_initial_session_update,
        })
    }

    /// Connect via WebSocket using the configured session.
    ///
    /// # Errors
    /// Returns an error if configuration is incomplete or the connection fails.
    pub async fn connect_ws(self) -> Result<super::Session> {
        self.build()?.connect_ws().await
    }
}

impl Default for RealtimeBuilder {
    fn default() -> Self {
        Self::new()
    }
}

pub struct VoiceSessionBuilder {
    inner: RealtimeBuilder,
}

impl VoiceSessionBuilder {
    #[must_use]
    fn new(mut inner: RealtimeBuilder) -> Self {
        let input = InputAudioConfig {
            format: Some(AudioFormat::pcm_24khz()),
            turn_detection: Some(crate::protocol::models::Nullable::Value(
                TurnDetection::ServerVad {
                    threshold: None,
                    prefix_padding_ms: None,
                    silence_duration_ms: None,
                    idle_timeout_ms: None,
                    create_response: Some(true),
                    interrupt_response: Some(true),
                },
            )),
            transcription: None,
            noise_reduction: None,
        };
        let output = OutputAudioConfig {
            format: Some(AudioFormat::pcm_24khz()),
            voice: None,
            speed: None,
        };
        inner.output_modalities = Some(OutputModalities::Audio);
        inner.audio = Some(AudioConfig {
            input: Some(input),
            output: Some(output),
        });
        inner.auto_barge_in = true;
        Self { inner }
    }

    #[must_use]
    pub fn api_key(mut self, key: impl Into<String>) -> Self {
        self.inner = self.inner.api_key(key);
        self
    }

    #[must_use]
    pub fn model(mut self, model: impl Into<String>) -> Self {
        self.inner = self.inner.model(model);
        self
    }

    #[must_use]
    pub fn call_id(mut self, call_id: impl Into<String>) -> Self {
        self.inner = self.inner.call_id(call_id);
        self
    }

    #[must_use]
    pub fn voice(mut self, voice: impl Into<String>) -> Self {
        self.inner = self.inner.voice(voice);
        if let Some(audio) = self.inner.audio.as_mut() {
            if let Some(output) = audio.output.as_mut() {
                output.voice = self
                    .inner
                    .voice
                    .clone()
                    .map(crate::protocol::models::Voice::from);
            }
        }
        self
    }

    #[must_use]
    pub fn instructions(mut self, instructions: impl Into<String>) -> Self {
        self.inner = self.inner.instructions(instructions);
        self
    }

    #[must_use]
    pub const fn vad_server_default(self) -> Self {
        let vad = TurnDetection::ServerVad {
            threshold: None,
            prefix_padding_ms: None,
            silence_duration_ms: None,
            idle_timeout_ms: None,
            create_response: Some(true),
            interrupt_response: Some(true),
        };
        self.set_turn_detection(vad)
    }

    #[must_use]
    pub const fn set_turn_detection(mut self, vad: TurnDetection) -> Self {
        if let Some(audio) = self.inner.audio.as_mut() {
            if let Some(input) = audio.input.as_mut() {
                input.turn_detection = Some(crate::protocol::models::Nullable::Value(vad));
            }
        }
        self
    }

    #[must_use]
    pub fn transcription(mut self, model: impl Into<String>) -> Self {
        let transcription = InputAudioTranscription {
            model: Some(model.into()),
            language: None,
            prompt: None,
        };
        if let Some(audio) = self.inner.audio.as_mut() {
            if let Some(input) = audio.input.as_mut() {
                input.transcription = Some(crate::protocol::models::Nullable::Value(transcription));
            }
        }
        self
    }

    #[must_use]
    pub const fn noise_reduction(mut self, noise_reduction: NoiseReduction) -> Self {
        if let Some(audio) = self.inner.audio.as_mut() {
            if let Some(input) = audio.input.as_mut() {
                input.noise_reduction =
                    Some(crate::protocol::models::Nullable::Value(noise_reduction));
            }
        }
        self
    }

    #[must_use]
    pub const fn auto_barge_in(mut self, enabled: bool) -> Self {
        self.inner.auto_barge_in = enabled;
        self
    }

    #[must_use]
    pub const fn auto_tool_response(mut self, enabled: bool) -> Self {
        self.inner.auto_tool_response = enabled;
        self
    }

    #[must_use]
    pub fn manual_sideband_control(mut self) -> Self {
        self.inner = self.inner.manual_sideband_control();
        self
    }

    #[must_use]
    pub fn tool_dispatcher(mut self, dispatcher: Arc<dyn ToolDispatcher>) -> Self {
        self.inner.dispatcher = Some(dispatcher);
        self
    }

    #[must_use]
    pub fn tools(mut self, tools: ToolRegistry) -> Self {
        self.inner = self.inner.tools(tools);
        self
    }

    #[must_use]
    pub fn tool<TArgs, TResp, F, Fut>(mut self, name: &str, handler: F) -> Self
    where
        TArgs: schemars::JsonSchema + serde::de::DeserializeOwned + Send + 'static,
        TResp: serde::Serialize + Send + 'static,
        F: Fn(TArgs) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = Result<TResp>> + Send + 'static,
    {
        self.inner = self.inner.tool(name, handler);
        self
    }

    #[must_use]
    pub fn tool_desc<TArgs, TResp, F, Fut>(
        mut self,
        name: &str,
        description: impl Into<String>,
        handler: F,
    ) -> Self
    where
        TArgs: schemars::JsonSchema + serde::de::DeserializeOwned + Send + 'static,
        TResp: serde::Serialize + Send + 'static,
        F: Fn(TArgs) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = Result<TResp>> + Send + 'static,
    {
        self.inner = self.inner.tool_desc(name, description, handler);
        self
    }

    #[must_use]
    pub fn tool_with_description<TArgs, TResp, F, Fut>(
        mut self,
        name: &str,
        description: impl Into<String>,
        handler: F,
    ) -> Self
    where
        TArgs: schemars::JsonSchema + serde::de::DeserializeOwned + Send + 'static,
        TResp: serde::Serialize + Send + 'static,
        F: Fn(TArgs) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = Result<TResp>> + Send + 'static,
    {
        self.inner = self.inner.tool_with_description(name, description, handler);
        self
    }

    #[must_use]
    pub fn on_text<F, Fut>(mut self, handler: F) -> Self
    where
        F: Fn(String) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = Result<()>> + Send + 'static,
    {
        self.inner = self.inner.on_text(handler);
        self
    }

    #[must_use]
    pub fn on_tool_call<F, Fut>(mut self, handler: F) -> Self
    where
        F: Fn(super::ToolCall) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = Result<super::ToolResult>> + Send + 'static,
    {
        self.inner = self.inner.on_tool_call(handler);
        self
    }

    #[must_use]
    pub fn on_raw_event<F, Fut>(mut self, handler: F) -> Self
    where
        F: Fn(crate::protocol::server_events::ServerEvent) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = Result<()>> + Send + 'static,
    {
        self.inner = self.inner.on_raw_event(handler);
        self
    }

    /// Connect via WebSocket using the configured voice session.
    ///
    /// # Errors
    /// Returns an error if configuration is incomplete or the connection fails.
    pub async fn connect_ws(self) -> Result<super::Session> {
        self.inner.connect_ws().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manual_sideband_control_builds_call_id_attach_without_initial_update() {
        let snapshot = RealtimeBuilder::new()
            .api_key("test-key")
            .model("gpt-realtime")
            .call_id("call_123")
            .manual_sideband_control()
            .build()
            .expect("builder snapshot");

        assert_eq!(snapshot.call_id.as_deref(), Some("call_123"));
        assert!(matches!(
            snapshot.connection_target(),
            super::super::session::SessionConnectTarget::CallId(call_id) if call_id == "call_123"
        ));
        assert!(!snapshot.send_initial_session_update);
        assert!(!snapshot.auto_barge_in);
        assert!(!snapshot.auto_tool_response);
    }
}
