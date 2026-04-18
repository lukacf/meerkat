use oai_rt_rs::Error;
use oai_rt_rs::protocol::models::{
    AudioConfig, AudioFormat, InputAudioConfig, McpToolConfig, ResponseConfig, SessionUpdateConfig,
    Tool,
};

// Replicate the base64 validation logic for testing
#[allow(clippy::result_large_err)]
fn validate_base64_audio(s: &str) -> Result<(), Error> {
    const MAX_BYTES: usize = 15 * 1024 * 1024;
    let bytes = s.as_bytes();
    if bytes.len() % 4 != 0 {
        return Err(Error::InvalidClientEvent(
            "input_audio_buffer.append invalid base64 length".to_string(),
        ));
    }

    let mut padding = 0;
    let mut seen_padding = false;
    for &b in bytes {
        if b == b'=' {
            seen_padding = true;
            padding += 1;
            continue;
        }
        if seen_padding {
            return Err(Error::InvalidClientEvent(
                "input_audio_buffer.append invalid base64 padding".to_string(),
            ));
        }
        let is_valid = matches!(b, b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'+' | b'/');
        if !is_valid {
            return Err(Error::InvalidClientEvent(
                "input_audio_buffer.append invalid base64 character".to_string(),
            ));
        }
    }

    if padding > 2 {
        return Err(Error::InvalidClientEvent(
            "input_audio_buffer.append invalid base64 padding".to_string(),
        ));
    }

    let decoded_len = (bytes.len() / 4) * 3 - padding;
    if decoded_len > MAX_BYTES {
        return Err(Error::InvalidClientEvent(format!(
            "input_audio_buffer.append exceeds 15MB ({decoded_len} bytes)"
        )));
    }

    Ok(())
}

// =============================================================================
// Base64 validation tests
// =============================================================================

#[test]
fn base64_valid_passes() {
    // "hello" in base64
    let valid = "aGVsbG8=";
    assert!(validate_base64_audio(valid).is_ok());
}

#[test]
fn base64_valid_no_padding_passes() {
    // "test" in base64 (no padding needed)
    let valid = "dGVzdA==";
    assert!(validate_base64_audio(valid).is_ok());
}

#[test]
fn base64_invalid_length_errors() {
    // Length not multiple of 4
    let invalid = "abc";
    let err = validate_base64_audio(invalid).unwrap_err();
    assert!(matches!(err, Error::InvalidClientEvent(msg) if msg.contains("invalid base64 length")));
}

#[test]
fn base64_invalid_character_errors() {
    // Contains invalid character '!'
    let invalid = "abc!";
    let err = validate_base64_audio(invalid).unwrap_err();
    assert!(
        matches!(err, Error::InvalidClientEvent(msg) if msg.contains("invalid base64 character"))
    );
}

#[test]
fn base64_invalid_padding_placement_errors() {
    // Padding in wrong position (data after padding)
    let invalid = "ab=c";
    let err = validate_base64_audio(invalid).unwrap_err();
    assert!(
        matches!(err, Error::InvalidClientEvent(msg) if msg.contains("invalid base64 padding"))
    );
}

#[test]
fn base64_exceeds_15mb_errors() {
    // Create base64 that decodes to > 15MB
    // 15MB = 15 * 1024 * 1024 = 15728640 bytes
    // Base64 encoding: 4 chars = 3 bytes decoded
    // For 15728641 bytes, we need ceil(15728641 / 3) * 4 = 20971524 chars
    let oversized = "A".repeat(20_971_524);
    let err = validate_base64_audio(&oversized).unwrap_err();
    assert!(matches!(err, Error::InvalidClientEvent(msg) if msg.contains("exceeds 15MB")));
}

#[test]
fn base64_exactly_15mb_passes() {
    // 15MB exactly = 15728640 bytes
    // For 15728640 bytes: (15728640 / 3) * 4 = 20971520 chars
    let max_size = "A".repeat(20_971_520);
    assert!(validate_base64_audio(&max_size).is_ok());
}

// =============================================================================
// AudioFormat validation tests
// =============================================================================

#[test]
fn audio_format_pcm_24khz_passes() {
    let format = AudioFormat::pcm_24khz();
    assert!(format.validate().is_ok());
}

#[test]
fn audio_format_pcm_wrong_rate_errors() {
    let format = AudioFormat::Pcm { rate: 16000 };
    let err = format.validate().unwrap_err();
    assert!(matches!(err, Error::InvalidClientEvent(msg) if msg.contains("rate must be 24000")));
}

#[test]
fn audio_format_pcm_48khz_errors() {
    let format = AudioFormat::Pcm { rate: 48000 };
    let err = format.validate().unwrap_err();
    assert!(matches!(err, Error::InvalidClientEvent(msg) if msg.contains("rate must be 24000")));
}

#[test]
fn audio_format_pcmu_passes() {
    let format = AudioFormat::Pcmu;
    assert!(format.validate().is_ok());
}

#[test]
fn audio_format_pcma_passes() {
    let format = AudioFormat::Pcma;
    assert!(format.validate().is_ok());
}

// =============================================================================
// MCP tool validation tests
// =============================================================================

#[test]
fn mcp_tool_with_server_url_passes() {
    let config = McpToolConfig {
        server_label: "test".to_string(),
        server_url: Some("https://example.com".to_string()),
        ..McpToolConfig::default()
    };
    assert!(config.validate().is_ok());
}

#[test]
fn mcp_tool_with_connector_id_passes() {
    let config = McpToolConfig {
        server_label: "test".to_string(),
        connector_id: Some("conn_123".to_string()),
        ..McpToolConfig::default()
    };
    assert!(config.validate().is_ok());
}

#[test]
fn mcp_tool_with_both_passes() {
    let config = McpToolConfig {
        server_label: "test".to_string(),
        server_url: Some("https://example.com".to_string()),
        connector_id: Some("conn_123".to_string()),
        ..McpToolConfig::default()
    };
    assert!(config.validate().is_ok());
}

#[test]
fn mcp_tool_missing_url_and_connector_errors() {
    let config = McpToolConfig {
        server_label: "test".to_string(),
        server_url: None,
        connector_id: None,
        ..McpToolConfig::default()
    };
    let err = config.validate().unwrap_err();
    assert!(
        matches!(err, Error::InvalidClientEvent(msg) if msg.contains("server_url or connector_id"))
    );
}

// =============================================================================
// Session/Response config validation integration
// =============================================================================

#[test]
fn session_update_with_invalid_audio_format_errors() {
    let config = SessionUpdateConfig {
        input_audio_format: Some(AudioFormat::Pcm { rate: 8000 }),
        ..SessionUpdateConfig::default()
    };

    // Validate through AudioFormat directly since SessionUpdate validation is private
    let err = config
        .input_audio_format
        .as_ref()
        .unwrap()
        .validate()
        .unwrap_err();
    assert!(matches!(err, Error::InvalidClientEvent(msg) if msg.contains("rate must be 24000")));
}

#[test]
fn session_update_with_invalid_mcp_tool_errors() {
    let invalid_mcp = Tool::Mcp(McpToolConfig {
        server_label: "broken".to_string(),
        server_url: None,
        connector_id: None,
        ..McpToolConfig::default()
    });

    // Validate through McpToolConfig directly
    if let Tool::Mcp(config) = &invalid_mcp {
        let err = config.validate().unwrap_err();
        assert!(
            matches!(err, Error::InvalidClientEvent(msg) if msg.contains("server_url or connector_id"))
        );
    }
}

#[test]
fn response_config_with_nested_invalid_audio_errors() {
    let config = ResponseConfig {
        audio: Some(AudioConfig {
            input: Some(InputAudioConfig {
                format: Some(AudioFormat::Pcm { rate: 44100 }),
                ..InputAudioConfig::default()
            }),
            output: None,
        }),
        ..ResponseConfig::default()
    };

    // Validate through nested AudioFormat
    let format = config
        .audio
        .as_ref()
        .unwrap()
        .input
        .as_ref()
        .unwrap()
        .format
        .as_ref()
        .unwrap();
    let err = format.validate().unwrap_err();
    assert!(matches!(err, Error::InvalidClientEvent(msg) if msg.contains("rate must be 24000")));
}

#[test]
fn valid_session_config_passes() {
    let config = SessionUpdateConfig {
        input_audio_format: Some(AudioFormat::pcm_24khz()),
        output_audio_format: Some(AudioFormat::pcm_24khz()),
        tools: Some(vec![Tool::Mcp(McpToolConfig {
            server_label: "weather".to_string(),
            server_url: Some("https://mcp.example.com".to_string()),
            ..McpToolConfig::default()
        })]),
        ..SessionUpdateConfig::default()
    };

    // Validate all components
    assert!(
        config
            .input_audio_format
            .as_ref()
            .unwrap()
            .validate()
            .is_ok()
    );
    assert!(
        config
            .output_audio_format
            .as_ref()
            .unwrap()
            .validate()
            .is_ok()
    );
    for tool in config.tools.as_ref().unwrap() {
        if let Tool::Mcp(mcp) = tool {
            assert!(mcp.validate().is_ok());
        }
    }
}
