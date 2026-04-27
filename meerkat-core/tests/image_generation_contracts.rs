#![allow(clippy::unwrap_used)]

use std::num::NonZeroU32;

use meerkat_core::lifecycle::run_primitive::ModelId;
use meerkat_core::{
    AssistantImageId, BlobId, BlobRef, GenerateImageRequest, ImageFormatPreference,
    ImageGenerationIntent, ImageGenerationTargetPreference, ImageQualityPreference,
    ImageSizePreference, MediaType, PromptSource, PromptText, ProviderId, ProviderImageMetadata,
    RevisedPromptDisposition, ToolCallId,
};
use uuid::Uuid;

fn uuid(n: u128) -> Uuid {
    Uuid::from_u128(n)
}

#[test]
fn public_image_generation_request_contract_roundtrips() {
    let request = GenerateImageRequest::new(
        ImageGenerationIntent::Generate {
            prompt: PromptText::new("draw a compact architecture diagram").unwrap(),
            prompt_source: PromptSource::ModelDistilled {
                tool_call_id: ToolCallId::new("tc_image"),
            },
            reference_images: Vec::new(),
        },
        ImageGenerationTargetPreference::Model {
            provider: ProviderId::new("openai"),
            model: ModelId::new("gpt-image-1"),
        },
        ImageSizePreference::Square1024,
        ImageQualityPreference::Medium,
        ImageFormatPreference::Png,
        NonZeroU32::new(1).unwrap(),
    )
    .unwrap();

    let encoded = serde_json::to_string(&request).unwrap();
    let decoded: GenerateImageRequest = serde_json::from_str(&encoded).unwrap();
    assert_eq!(decoded, request);
}

#[test]
fn public_assistant_image_block_contract_roundtrips_without_inline_bytes() {
    let block = meerkat_core::AssistantBlock::Image {
        image_id: AssistantImageId::new(uuid(0x51)),
        blob_ref: BlobRef {
            blob_id: BlobId::new("blob-generated-image"),
            media_type: "image/png".into(),
        },
        media_type: MediaType::new("image/png"),
        width: 1024,
        height: 1024,
        revised_prompt: RevisedPromptDisposition::NotRequested,
        meta: ProviderImageMetadata::NotEmitted,
    };

    let encoded = serde_json::to_value(&block).unwrap();
    assert_eq!(encoded["block_type"], "image");
    assert_eq!(
        encoded["data"]["blob_ref"]["blob_id"],
        "blob-generated-image"
    );
    assert!(
        encoded.to_string().contains("blob-generated-image"),
        "durable blob id should be the persisted source of truth"
    );
    assert!(
        !encoded.to_string().contains("base64"),
        "assistant image blocks must not require inline provider bytes"
    );

    let decoded: meerkat_core::AssistantBlock = serde_json::from_value(encoded).unwrap();
    assert_eq!(decoded, block);
}
