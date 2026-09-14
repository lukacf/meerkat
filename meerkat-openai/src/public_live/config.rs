//! Public provider configuration lowering. No executor instructions or tool
//! definitions are accepted here; the managed backend has one fixed function.

use meerkat_llm_core::provider_runtime::{ResolvedLiveExecution, ResolvedLiveTarget};
use oai_rt_rs::live::{
    AudioConfig, AudioFormat, AudioOutput, ClientConfig, DataChannelConfig, DelegationConfig,
    EventPermissions, Field, ResponsesConfig, ResponsesOptions, ServerEventSelector, SessionConfig,
    Tool, Voice,
};

use super::context::LIVE_PROFILE_INSTRUCTIONS_MAX_BYTES;
use super::request::{INVOKE_MEERKAT, InvokeMeerkatRequest};

/// Content settings from the owning voice profile. This is not an activation
/// permit and cannot select another model, backend, credential, or executor.
#[derive(Clone, Copy, Default)]
pub struct PublicLiveVoiceSettings<'a> {
    pub voice: Option<&'a str>,
    pub instructions: Option<&'a str>,
}

impl std::fmt::Debug for PublicLiveVoiceSettings<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PublicLiveVoiceSettings")
            .field("voice_selected", &self.voice.is_some())
            .field("instruction_bytes", &self.instructions.map(str::len))
            .finish()
    }
}

pub fn session_config(
    target: &ResolvedLiveTarget,
    settings: PublicLiveVoiceSettings<'_>,
) -> Result<SessionConfig, PublicLiveConfigError> {
    if target.voice_identity().provider_params.is_some() {
        return Err(PublicLiveConfigError::VoiceParametersUnsupported);
    }
    lower_session_config(
        target.voice_identity().model.as_str(),
        target.execution(),
        settings,
    )
}

fn lower_session_config(
    voice_model: &str,
    execution: &ResolvedLiveExecution,
    settings: PublicLiveVoiceSettings<'_>,
) -> Result<SessionConfig, PublicLiveConfigError> {
    if settings
        .instructions
        .is_some_and(|instructions| instructions.len() > LIVE_PROFILE_INSTRUCTIONS_MAX_BYTES)
    {
        return Err(PublicLiveConfigError::InstructionsTooLarge);
    }
    if settings
        .voice
        .is_some_and(|voice| voice.is_empty() || voice.len() > 128)
    {
        return Err(PublicLiveConfigError::InvalidVoice);
    }
    let delegation = match execution {
        ResolvedLiveExecution::ClientContext { .. } => DelegationConfig::Client,
        ResolvedLiveExecution::FunctionBridge { backend } => {
            let parameters = InvokeMeerkatRequest::parameters_schema()
                .as_object()
                .cloned()
                .ok_or(PublicLiveConfigError::InvalidManagedFunctionSchema)?;
            DelegationConfig::Responses {
                responses: ResponsesConfig {
                    model: backend.model().to_owned(),
                    options: ResponsesOptions {
                        tools: Some(vec![Tool::Function {
                            name: INVOKE_MEERKAT.into(),
                            description: Field::Value("Submit a request to the configured Meerkat executor. The host controls execution permission.".into()),
                            parameters: Field::Value(parameters),
                            strict: Field::Value(true),
                        }]),
                        ..ResponsesOptions::default()
                    },
                },
            }
        }
    };
    Ok(SessionConfig {
        model: voice_model.to_owned(),
        audio: Some(AudioConfig {
            format: Some(AudioFormat::Pcm { rate: 24_000 }),
            output: settings.voice.map(|voice| AudioOutput {
                voice: Some(Voice::Named(voice.to_owned())),
            }),
        }),
        client: Some(ClientConfig {
            data_channel: DataChannelConfig {
                allowed_client_events: Some(EventPermissions::Selected(Vec::new())),
                allowed_server_events: Some(EventPermissions::Selected(vec![
                    ServerEventSelector {
                        event_type: "session.input_transcript.delta".into(),
                        response_event: None,
                    },
                    ServerEventSelector {
                        event_type: "session.output_transcript.delta".into(),
                        response_event: None,
                    },
                ])),
            },
        }),
        delegation: Field::Value(delegation),
        input: None,
        instructions: settings
            .instructions
            .map_or(Field::Absent, |value| Field::Value(value.to_owned())),
        store: None,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PublicLiveConfigError {
    #[error("public Live voice provider_params are unsupported; use the voice profile settings")]
    VoiceParametersUnsupported,
    #[error("public Live voice instructions exceed the local UTF-8 byte bound")]
    InstructionsTooLarge,
    #[error("public Live voice name must contain 1-128 UTF-8 bytes")]
    InvalidVoice,
    #[error("managed public Live function schema is not an object")]
    InvalidManagedFunctionSchema,
}

#[cfg(test)]
mod tests {
    use super::*;
    use meerkat_core::live_execution::profile::{LiveClientRequestPolicy, LiveManagedBackendModel};
    use meerkat_core::{Config, ModelRegistry, Provider};
    use oai_rt_rs::live::{ClientEvent, Codec, Command};
    use serde_json::json;

    #[test]
    fn lowering_keeps_models_tools_instructions_and_browser_permissions_separate()
    -> Result<(), Box<dyn std::error::Error>> {
        let registry = ModelRegistry::from_config(&Config::default(), meerkat_models::canonical())?;
        let identity = meerkat_core::SessionLlmIdentity {
            model: "gpt-5.5".into(),
            provider: Provider::OpenAI,
            self_hosted_server_id: None,
            provider_params: None,
            auth_binding: None,
        };
        let backend = registry
            .profile_witness_for_provider(identity.provider, &identity.model)
            .ok_or("missing managed backend profile")?;
        let modes = [
            ResolvedLiveExecution::ClientContext {
                request_policy: LiveClientRequestPolicy::SnapshotAtDelegation,
            },
            ResolvedLiveExecution::function_bridge(
                &LiveManagedBackendModel {
                    provider: Provider::OpenAI,
                    model: "gpt-5.5".into(),
                },
                backend,
            )?,
        ];
        for (index, execution) in modes.iter().enumerate() {
            let config = lower_session_config(
                "gpt-live-1",
                execution,
                PublicLiveVoiceSettings {
                    voice: Some("marin"),
                    instructions: Some("voice-only instructions"),
                },
            )?;
            let wire: serde_json::Value = serde_json::from_str(
                &Codec::default().encode(&ClientEvent::new(Command::Start { session: config }))?,
            )?;
            assert_eq!(wire["session"]["model"], "gpt-live-1");
            assert_eq!(wire["session"]["instructions"], "voice-only instructions");
            assert_eq!(
                wire["session"]["client"]["data_channel"]["allowed_client_events"],
                json!([])
            );
            assert_eq!(
                wire["session"]["client"]["data_channel"]["allowed_server_events"],
                json!([
                    {"type":"session.input_transcript.delta"},{"type":"session.output_transcript.delta"}
                ])
            );
            assert_eq!(
                wire["session"]["audio"]["format"],
                json!({"type":"audio/pcm","rate":24000})
            );
            assert!(wire["session"].get("input").is_none());
            if index == 0 {
                assert_eq!(wire["session"]["delegation"], json!({"type":"client"}));
            } else {
                let backend = &wire["session"]["delegation"]["responses"];
                assert_eq!(backend["model"], "gpt-5.5");
                assert!(backend.get("instructions").is_none());
                assert_eq!(backend["tools"].as_array().ok_or("tools")?.len(), 1);
                assert_eq!(backend["tools"][0]["name"], INVOKE_MEERKAT);
                assert_eq!(
                    backend["tools"][0]["parameters"],
                    InvokeMeerkatRequest::parameters_schema()
                );
            }
        }
        Ok(())
    }

    #[test]
    fn settings_preserve_omission_and_reject_bounds_without_a_smaller_retry()
    -> Result<(), PublicLiveConfigError> {
        let mode = ResolvedLiveExecution::ClientContext {
            request_policy: LiveClientRequestPolicy::ExplicitApplicationRequest,
        };
        let omitted =
            lower_session_config("gpt-live-1", &mode, PublicLiveVoiceSettings::default())?;
        assert!(matches!(omitted.instructions, Field::Absent));
        let oversized = "x".repeat(8193);
        for settings in [
            PublicLiveVoiceSettings {
                voice: Some(""),
                instructions: None,
            },
            PublicLiveVoiceSettings {
                voice: None,
                instructions: Some(&oversized),
            },
        ] {
            assert!(lower_session_config("gpt-live-1", &mode, settings).is_err());
        }
        Ok(())
    }
}
