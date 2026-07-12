from typing import NotRequired, Required, get_args, get_origin, get_type_hints

from meerkat.generated.types import (
    BlobRef,
    ForkSessionReplaceParams,
    ProviderImageMetadata,
    ProviderImageMetadataOpenai,
    RevisedPromptDisposition,
    RewriteSessionTranscriptParams,
    TranscriptRewriteMessage,
    TranscriptRewriteReason,
    TranscriptRewriteSelection,
    WireAssistantBlock,
    WireAssistantBlockImageData,
    WireAssistantBlockReasoningData,
    WireAssistantBlockServerToolContentData,
    WireAssistantBlockTextData,
    WireContentBlock,
    WireContentBlockImageBlob,
    WireContentBlockImageInline,
    WireContentBlockVideoInline,
    WireContentBlockVideoUri,
    WireProviderMeta,
    WireProviderMetaAnthropic,
    WireProviderMetaAnthropicCompaction,
    WireProviderMetaAnthropicRedacted,
    WireProviderMetaGemini,
    WireProviderMetaOpenAi,
    WireProviderMetaOpenAiResponse,
    WireProviderMetaUnknown,
    WireTranscriptReplacement,
)


def test_fork_replace_keeps_closed_generated_replacement_graph() -> None:
    hints = get_type_hints(ForkSessionReplaceParams)
    assert hints["replacement"] == WireTranscriptReplacement

    replacement_variants = {
        variant.__name__ for variant in get_args(WireTranscriptReplacement)
    }
    assert replacement_variants == {
        "WireTranscriptReplacementMessage",
        "WireTranscriptReplacementUserContentBlock",
        "WireTranscriptReplacementAssistantBlock",
        "WireTranscriptReplacementToolResultContentBlock",
    }
    assert get_args(TranscriptRewriteMessage)
    assert get_args(WireAssistantBlock)

    rewrite_hints = get_type_hints(RewriteSessionTranscriptParams)
    assert rewrite_hints["reason"] is TranscriptRewriteReason
    assert rewrite_hints["selection"] == TranscriptRewriteSelection


def test_generated_content_blocks_preserve_flattened_source_payloads() -> None:
    content_variants = set(get_args(WireContentBlock))
    assert {
        WireContentBlockImageInline,
        WireContentBlockImageBlob,
        WireContentBlockVideoInline,
        WireContentBlockVideoUri,
    }.issubset(content_variants)

    expected_required = {
        WireContentBlockImageInline: {"type", "media_type", "source", "data"},
        WireContentBlockImageBlob: {"type", "media_type", "source", "blob_id"},
        WireContentBlockVideoInline: {
            "type",
            "media_type",
            "duration_ms",
            "source",
            "data",
        },
        WireContentBlockVideoUri: {
            "type",
            "media_type",
            "duration_ms",
            "source",
            "uri",
        },
    }
    for variant, required in expected_required.items():
        hints = get_type_hints(variant, include_extras=True)
        assert required.issubset(hints)
        assert all(get_origin(hints[field]) is Required for field in required)


def test_generated_assistant_image_keeps_closed_nested_contracts() -> None:
    hints = get_type_hints(WireAssistantBlockImageData)
    assert hints["blob_ref"] is BlobRef
    assert hints["meta"] == ProviderImageMetadata
    assert hints["revised_prompt"] == RevisedPromptDisposition


def test_generated_provider_metadata_is_closed_and_schema_nullable() -> None:
    variants = set(get_args(WireProviderMeta))
    assert variants == {
        WireProviderMetaAnthropic,
        WireProviderMetaAnthropicRedacted,
        WireProviderMetaAnthropicCompaction,
        WireProviderMetaGemini,
        WireProviderMetaOpenAi,
        WireProviderMetaOpenAiResponse,
        WireProviderMetaUnknown,
    }

    required_payloads = {
        WireProviderMetaAnthropic: "signature",
        WireProviderMetaAnthropicRedacted: "data",
        WireProviderMetaAnthropicCompaction: "content",
        WireProviderMetaGemini: "thoughtSignature",
        WireProviderMetaOpenAi: "id",
        WireProviderMetaOpenAiResponse: "response_id",
    }
    for variant, payload in required_payloads.items():
        hints = get_type_hints(variant, include_extras=True)
        assert get_origin(hints["provider"]) is Required
        assert get_origin(hints[payload]) is Required

    open_ai_hints = get_type_hints(WireProviderMetaOpenAi, include_extras=True)
    for field in ("encrypted_content", "phase", "response_id"):
        assert get_origin(open_ai_hints[field]) is NotRequired
        nullable = get_args(open_ai_hints[field])[0]
        assert set(get_args(nullable)) == {str, type(None)}


def test_promoted_optional_nullable_fields_keep_null_as_a_value() -> None:
    provider_variants = set(get_args(WireProviderMeta))

    text_hints = get_type_hints(WireAssistantBlockTextData, include_extras=True)
    assert get_origin(text_hints["meta"]) is NotRequired
    nullable_meta = get_args(text_hints["meta"])[0]
    assert type(None) in get_args(nullable_meta)
    assert provider_variants.issubset(set(get_args(nullable_meta)))

    server_hints = get_type_hints(
        WireAssistantBlockServerToolContentData,
        include_extras=True,
    )
    assert get_origin(server_hints["id"]) is NotRequired
    assert set(get_args(get_args(server_hints["id"])[0])) == {str, type(None)}

    image_hints = get_type_hints(
        ProviderImageMetadataOpenai,
        include_extras=True,
    )
    assert get_origin(image_hints["response_id"]) is NotRequired
    assert set(get_args(get_args(image_hints["response_id"])[0])) == {
        str,
        type(None),
    }

    reasoning_hints = get_type_hints(
        WireAssistantBlockReasoningData,
        include_extras=True,
    )
    assert get_origin(reasoning_hints["text"]) is NotRequired
    assert get_args(reasoning_hints["text"])[0] is str
