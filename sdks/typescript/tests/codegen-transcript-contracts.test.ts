import type {
  BlobRef,
  ForkSessionReplaceParams,
  ProviderImageMetadata,
  RevisedPromptDisposition,
  RewriteSessionTranscriptParams,
  TranscriptRewriteReason,
  TranscriptRewriteSelection,
  TranscriptRewriteMessage,
  WireAssistantBlock,
  WireContentBlock,
  WireProviderMeta,
  WireTranscriptReplacement,
} from "../src/generated/types.js";

const inlineImage: WireContentBlock = {
  type: "image",
  media_type: "image/png",
  source: "inline",
  data: "aGVsbG8=",
};

const blobImage: WireContentBlock = {
  type: "image",
  media_type: "image/png",
  source: "blob",
  blob_id: "blob-1",
};

const uriVideo: WireContentBlock = {
  type: "video",
  media_type: "video/mp4",
  duration_ms: 1000,
  source: "uri",
  uri: "https://media.example/video.mp4",
};

const blobRef: BlobRef = { blob_id: "blob-2", media_type: "image/webp" };
const providerMeta: ProviderImageMetadata = {
  provider: "openai",
  target_model: "gpt-image-1",
};
const revisedPrompt: RevisedPromptDisposition = {
  disposition: "revised",
  source: "provider",
  text: { content: "revised" },
};
const assistantImage: WireAssistantBlock = {
  block_type: "image",
  data: {
    image_id: "image-1",
    blob_ref: blobRef,
    media_type: "image/webp",
    width: 1024,
    height: 1024,
    revised_prompt: revisedPrompt,
    meta: providerMeta,
  },
};

const replacement: WireTranscriptReplacement = {
  type: "assistant_block",
  block_index: 0,
  block: assistantImage,
};
const params: ForkSessionReplaceParams = {
  session_id: "session-1",
  message_index: 0,
  replacement,
};

const rewriteReason: TranscriptRewriteReason = { kind: "operator_edit" };
const rewriteSelection: TranscriptRewriteSelection = {
  type: "edit_message_range",
  range: { start: 0, end: 1 },
};
const rewriteParams: RewriteSessionTranscriptParams = {
  session_id: "session-1",
  selection: rewriteSelection,
  replacement: [
    {
      role: "user",
      content: "replacement",
      transcript_role: "injected_context",
    },
  ],
  reason: rewriteReason,
};

const providerMetas: WireProviderMeta[] = [
  { provider: "anthropic", signature: "sig" },
  { provider: "anthropic_redacted", data: "redacted" },
  { provider: "anthropic_compaction", content: "compact" },
  { provider: "gemini", thoughtSignature: "thought" },
  {
    provider: "open_ai",
    id: "item-1",
    encrypted_content: null,
    phase: null,
    response_id: null,
  },
  { provider: "open_ai_response", response_id: "response-1" },
  { provider: "unknown" },
];

const nullableAssistantMeta: WireAssistantBlock = {
  block_type: "text",
  data: { text: "answer", meta: null },
};
const nullableServerToolId: WireAssistantBlock = {
  block_type: "server_tool_content",
  data: {
    id: null,
    kind: { kind: "web_search" },
    content: {},
    meta: null,
  },
};
const nullableImageProviderMeta: ProviderImageMetadata = {
  provider: "openai",
  target_model: "gpt-image-1",
  response_id: null,
  image_generation_call_id: null,
};
const nullableRewriteMessage: TranscriptRewriteMessage = {
  role: "system_notice",
  kind: "generic",
  body: null,
  created_at: null,
};

// @ts-expect-error anthropic metadata requires its signature payload.
const invalidAnthropicMeta: WireProviderMeta = { provider: "anthropic" };
// @ts-expect-error provider metadata is a closed wire vocabulary.
const invalidProviderMeta: WireProviderMeta = { provider: "future" };
const invalidReasoningNull: WireAssistantBlock = {
  block_type: "reasoning",
  // @ts-expect-error omission is allowed, but explicit null is not in this schema.
  data: { text: null },
};

// @ts-expect-error flattened inline image payload requires data.
const invalidInlineImage: WireContentBlock = {
  type: "image",
  media_type: "image/png",
  source: "inline",
};

const invalidBlobImage: WireContentBlock = {
  type: "image",
  media_type: "image/png",
  source: "blob",
  // @ts-expect-error blob image cannot substitute the inline data carrier.
  data: "not-a-blob-id",
};

void inlineImage;
void blobImage;
void uriVideo;
void params;
void rewriteParams;
void providerMetas;
void nullableAssistantMeta;
void nullableServerToolId;
void nullableImageProviderMeta;
void nullableRewriteMessage;
void invalidAnthropicMeta;
void invalidProviderMeta;
void invalidReasoningNull;
void invalidInlineImage;
void invalidBlobImage;
