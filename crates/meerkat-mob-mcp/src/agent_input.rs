//! Decoding of model-supplied mob tool arguments.
//!
//! Agent-facing tools decode through the public input contract, plus
//! refusals the host-facing surfaces do not make: a model may not name a host
//! filesystem path as a skill source; may not reference a stored blob by id,
//! because the blob store has no fact showing the calling session may read
//! it; and may not reference a video by URI, which the provider would fetch
//! with the host's credentials. Inline skill content and inline image and
//! video bytes still work.

use meerkat_contracts::wire::{
    MobDefinitionInput, MobSkillSourceInput, WireContentBlock, WireContentInput, WireImageData,
    WireVideoData,
};
use meerkat_core::types::ContentInput;
use meerkat_mob::MobDefinition;

/// Decode a model-supplied mob definition: the public contract, without
/// host-path skill sources.
pub(crate) fn decode_agent_mob_definition(
    input: MobDefinitionInput,
) -> Result<MobDefinition, String> {
    if let Some(name) = input.skills.iter().find_map(|(name, source)| {
        matches!(source, MobSkillSourceInput::Path { .. }).then_some(name)
    }) {
        return Err(format!(
            "skill '{name}' names a host filesystem path; a model-supplied definition may define \
             skills inline or reference the host's skills, not read host paths"
        ));
    }
    crate::decode_public_mob_definition(input)
}

/// Decode model-supplied content: the public contract, without stored-blob
/// or provider-fetched references.
pub(crate) fn decode_agent_content_input(input: WireContentInput) -> Result<ContentInput, String> {
    if let WireContentInput::Blocks(blocks) = &input {
        for block in blocks {
            match block {
                WireContentBlock::Image {
                    data: WireImageData::Blob { .. },
                    ..
                } => {
                    return Err(
                        "an image may not reference a stored blob by id from a model-supplied \
                         message; send the image bytes inline"
                            .to_string(),
                    );
                }
                WireContentBlock::Video {
                    data: WireVideoData::Uri { .. },
                    ..
                } => {
                    return Err(
                        "a video may not reference a URI from a model-supplied message, because \
                         the provider would fetch it with the host's credentials; send the video \
                         bytes inline"
                            .to_string(),
                    );
                }
                _ => {}
            }
        }
    }
    ContentInput::try_from(input).map_err(str::to_string)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use serde_json::json;

    fn definition(skill: serde_json::Value) -> MobDefinitionInput {
        serde_json::from_value(json!({
            "id": "skills",
            "profiles": { "worker": { "model": "claude-sonnet-4-5", "skills": ["s"] } },
            "skills": { "s": skill }
        }))
        .unwrap()
    }

    #[test]
    fn a_host_path_skill_source_is_refused_and_inline_content_is_kept() {
        let error = decode_agent_mob_definition(definition(
            json!({ "source": "path", "path": "/etc/passwd" }),
        ))
        .expect_err("a model-supplied host path is refused");
        assert!(error.contains("host filesystem path"), "{error}");
        let decoded = decode_agent_mob_definition(definition(
            json!({ "source": "inline", "content": "be brief" }),
        ))
        .expect("inline skill content is model-authored text");
        assert_eq!(decoded.skills.len(), 1);
    }

    #[test]
    fn a_stored_blob_reference_is_refused_and_inline_bytes_are_kept() {
        let blob: WireContentInput = serde_json::from_value(json!([
            { "type": "text", "text": "look" },
            { "type": "image", "media_type": "image/png", "source": "blob", "blob_id": "sha256:abc" }
        ]))
        .unwrap();
        let error = decode_agent_content_input(blob).expect_err("a blob reference is refused");
        assert!(error.contains("stored blob"), "{error}");
        let inline: WireContentInput = serde_json::from_value(json!([
            { "type": "image", "media_type": "image/png", "source": "inline", "data": "iVBORw0KGgo=" }
        ]))
        .unwrap();
        decode_agent_content_input(inline).expect("inline image bytes are accepted");
    }

    #[test]
    fn a_video_uri_is_refused_and_inline_bytes_are_kept() {
        let uri: WireContentInput = serde_json::from_value(json!([
            { "type": "video", "media_type": "video/mp4", "duration_ms": 1000,
              "source": "uri", "uri": "gs://host-bucket/private.mp4" }
        ]))
        .unwrap();
        let error = decode_agent_content_input(uri).expect_err("a video URI is refused");
        assert!(error.contains("host's credentials"), "{error}");
        let inline: WireContentInput = serde_json::from_value(json!([
            { "type": "video", "media_type": "video/mp4", "duration_ms": 1000,
              "source": "inline", "data": "AAAAIGZ0eXA=" }
        ]))
        .unwrap();
        decode_agent_content_input(inline).expect("inline video bytes are accepted");
    }
}
