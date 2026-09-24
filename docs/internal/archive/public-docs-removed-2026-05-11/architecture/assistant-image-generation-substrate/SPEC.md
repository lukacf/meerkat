# Assistant Image Generation Substrate Specification

Status: Draft v1 (Meerkat internal)

Purpose: Define the normative service/runtime contract for assistant-only image
generation in Meerkat.

## Normative Language

The key words `MUST`, `MUST NOT`, `REQUIRED`, `SHOULD`, `SHOULD NOT`,
`RECOMMENDED`, `MAY`, and `OPTIONAL` in this document are to be interpreted as
described in RFC 2119.

All variability MUST be represented as typed machine, provider-profile, or seam
facts. The implementation MUST NOT rely on undocumented local policy.

## 1. Problem Statement

Meerkat needs image generation that is visible to users as normal assistant
output while remaining faithful to runtime dogma:

- one semantic fact has one owner;
- machines own lifecycle;
- shell code owns IO mechanics;
- provider crates own provider-specific model, parameter, and wire facts;
- surfaces render state and content but do not own meaning;
- projections are rebuildable views, never source truth.

The hard part is not that providers can generate images. The hard part is that
generated images must become canonical transcript content, later image edits
must select exact transcript image blocks, provider-specific image capabilities
must not leak into runtime helpers, and tool-call/tool-result adjacency must
remain valid for model providers.

Important boundary:

- `generate_image` is an assistant-only tool operation.
- No CLI, SDK, REST, RPC, MCP, or user surface may invoke `generate_image`.
- Users provide image input as session content.
- The assistant decides whether to call `generate_image`.
- OpenAI and Gemini are the v1 provider targets.

## 2. Goals and Non-Goals

### 2.1 Goals

- Represent user, assistant, and tool-result images as canonical transcript
  `ContentBlock::Image`.
- Remove the separate block-assistant transcript ontology.
- Add `created_at` to every canonical transcript message.
- Resolve image sources only through explicit machine-scoped source tokens.
- Allow generated-image edits to use user images, prior assistant images, and
  prior tool-result images.
- Keep provider-specific image model ownership, parameter schemas, backend
  choice facts, request DTOs, and response parsing in provider crates.
- Keep lifecycle, admission, source resolution, target resolution, barrier
  membership, publication, and terminalization in `MeerkatMachine`.
- Publish generated image content after the matching tool result without
  breaking tool-call/tool-result adjacency.
- Preserve truthful crash recovery around provider sends and transcript
  publication.
- Expose generated images to public surfaces and SDKs only as transcript content
  and blob retrieval.

### 2.2 Non-Goals

- Direct public image-generation APIs.
- Surface-local image handles.
- Prompt-string source inference such as "the previous image".
- Passive implicit image replay into provider requests.
- Persistent source tokens across invocations.
- Selection of compacted-away images.
- Provider fallback after a backend has been selected.
- Background image generation.
- Backwards compatibility with old transcript rows.

## 3. System Overview

### 3.1 Main Components

1. `Assistant Invocation Preparation`
   - Creates a model invocation boundary.
   - Offers assistant-visible tools and source tokens.
   - Fails before model invocation if tool/source manifest creation fails.

2. `MeerkatMachine`
   - Owns image operation lifecycle and terminal meaning.
   - Owns assistant-only admission, target resolution, source resolution,
     provider-input decisions, barrier membership, batch commit planning, and
     publish outcomes.

3. `Transcript Store`
   - Stores canonical messages, blocks, commit receipts, and visibility facts.
   - Supplies transcript commit evidence to machine decisions.

4. `Blob Store`
   - Stores bytes and supplies mechanical storage evidence.
   - Does not own content identity or source eligibility.

5. `Provider Profile Seam`
   - Supplies typed provider-local model, backend, capability, parameter, and
     request-building facts from provider crates.
   - Owns no provider facts itself.

6. `Provider IO Shell`
   - Performs provider HTTP/API calls from machine-approved prepared artifacts.
   - Delegates provider wire parsing to provider crates.
   - Reports provider-crate-produced typed evidence to `MeerkatMachine`.

7. `Public Surfaces and SDKs`
   - Send user image content.
   - Render assistant image content.
   - Retrieve blobs.
   - Expose no direct `generate_image` call.

### 3.2 Authority Boundaries

- `MeerkatMachine` MUST own semantic lifecycle state.
- Provider crates MUST own provider-specific facts and wire shapes.
- Shell code MUST own byte IO, provider calls, and durable evidence writes.
- Surfaces MUST be skins over canonical transcript and machine state.
- No helper, SDK, request builder, or surface MAY infer semantic source,
  provider target, retry, or terminal meaning.

### 3.3 External Dependencies

- Provider APIs for OpenAI and Gemini.
- Provider credentials surfaced as AuthMachine evidence.
- Durable transcript storage.
- Durable blob or artifact storage.
- Existing assistant tool-batch protocol.

## 4. Core Domain Model

### 4.1 Transcript Messages

Canonical transcript messages:

- `SystemMessage`
- `SystemNoticeMessage`
- `UserMessage`
- `AssistantMessage`
- `ToolResultsMessage`

Every canonical transcript message MUST contain:

- `message_id`
- `created_at`

`created_at` is display metadata. It MUST NOT be used as lifecycle truth or
ordering authority. Core constructors SHOULD stamp ordinary messages from
`MessageTimestamp::now`. Tests, recovery, and replay paths MAY pass explicit
timestamps.

`UserMessage` MUST contain ordered `ContentBlockEntry` values and
`UserMessageMeta`.

`AssistantMessage` MUST contain ordered `AssistantBlock` values and
`AssistantMessageMeta`.

`ToolResultsMessage` MUST contain ordered `ToolResult` values.

There MUST NOT be a canonical `Message::BlockAssistant` row.

### 4.2 Content Blocks

`ContentBlockEntry` fields:

- `block_id`
- `block`

`ContentBlock` variants:

- `Text`
- `Image`

`ContentBlock::Image` MUST be the only visible transcript image shape for user,
assistant, and tool-result media.

Image data MUST name transcript-visible content identity, not raw storage
identity. Raw blob ids are shell mechanics and MUST NOT be accepted as image
sources.

### 4.3 Assistant Blocks

`AssistantBlock` variants:

- `Content(ContentBlockEntry)`
- `ToolUse`
- `Reasoning`

There MUST NOT be an `AssistantBlock::Image` variant. Assistant media MUST be
represented as `AssistantBlock::Content(ContentBlock::Image)`.

Provider replay refs, reasoning data, usage, warnings, provider ids, revised
prompts, and traces are audit or replay facts. They MUST NOT be visible content.

### 4.4 Source Tokens

`SourceToken` is an opaque model-facing value scoped to one assistant invocation
manifest.

Source tokens MUST cite transcript commit truth through a durable
`SourceTokenManifest`.

Eligible source content:

- user message image blocks;
- assistant message image blocks;
- tool-result image blocks.

Tokens MUST be valid only for tool calls from the invocation that received the
manifest and only through that invocation's batch admission window. Prior tokens
MUST be rejected outside their manifest.

Labels and prose around tokens MAY help the assistant choose, but labels MUST
NOT be source authority.

### 4.5 Image Request

The model-facing request MUST contain universal fields plus opaque provider
parameters:

- `prompt`
- `kind`
- `target`
- `provider_params`
- `output`
- `source_images`
- `reference_images`

`source_images` are edit bases. `Edit` operations MUST include them, and
`Generate` operations MUST reject them. `reference_images` are non-edited
guidance images. Duplicate tokens and cross-role reuse MUST fail through typed
machine denial.

`target` MUST be either:

- explicit provider and model;
- provider default.

Provider identity MUST NOT be inferred from model-name strings.

`provider_params` MUST be opaque to core and runtime before provider
normalization. Opaque does not mean unvalidated: provider crates MUST normalize
raw JSON against provider-owned schemas before request preparation.

`output` MUST include explicit count, format, size intent, and quality intent.
No silent output defaults are allowed.

### 4.6 Provider Profiles

Provider crates MUST own `ModelProfile` facts:

- supported image model names;
- provider-local default image model;
- backend identities and defaults;
- backend candidate facts;
- parameter schemas and docs;
- request serialization;
- response parsing.

The runtime factory MAY compose typed provider-profile readers, but it MUST NOT
hard-code provider model-name conventions, default image models, backend
selection rules, or provider parameter schemas.

## 5. Assistant Invocation Contract

`generate_image` MUST be offered only inside an assistant invocation.

Before the model is called, `MeerkatMachine` MUST record an
`AssistantInvocationOffer` containing the tool manifest and source-token
manifest.

If the manifest cannot be created, the invocation MUST terminalize before the
model is called.

If any non-assistant caller attempts to invoke `generate_image`, the operation
MUST fail through typed machine denial.

The model-visible tool schema MUST expose:

- universal image operation fields;
- explicit source token arrays;
- opaque provider params.

It MUST NOT expose public blob handles or surface handles.

## 6. Source Resolution Contract

Source resolution is machine-owned and transcript-only.

The resolver MUST check only manifest facts:

- token exists in the active manifest;
- referenced message/block is inside the invocation boundary;
- block is visible;
- block contains `ContentBlock::Image`;
- image data is blob-backed transcript content.

The resolver MUST NOT:

- inspect prompt strings;
- consult provider adapters;
- accept raw blob ids;
- accept surface handles;
- read blob storage;
- decide blob reachability;
- resurrect compacted-away images.

Invalid, invisible, compacted-away, non-image, non-blob, or duplicate/cross-role
source selections MUST fail through typed source-resolution denial.

Blob reachability is a later hydration concern. Missing, unreachable, or
digest-mismatched bytes MUST fail through typed hydration evidence.

## 7. Target and Parameter Resolution

Target resolution is machine-owned.

Inputs:

- assistant target intent;
- provider profile snapshot;
- session image policy snapshot;
- AuthMachine identity evidence.

The machine MUST record one `ImageTargetResolutionDecision` for provider, model,
backend, policy, and auth admission.

Provider defaults MUST come from provider profiles. Provider input observations
MUST be emitted only after target resolution.

Provider parameter normalization MUST happen after target resolution and before
provider request preparation. Provider normalization MUST return typed evidence
containing schema revision, digest, effective values, origins, and conflicts.

Provider parameter schemas MUST reject duplicates of universal output fields.

Request builders MUST serialize only accepted provider values and MUST NOT make
semantic denial decisions.

## 8. Image Operation State Machine

The v1 lifecycle MUST be represented in the catalog `MeerkatMachine`, not in a
helper authority.

Required phases:

1. assistant tool-batch admission;
2. universal request validation;
3. target resolution;
4. provider parameter normalization evidence;
5. transcript source resolution;
6. source hydration request and evidence observation;
7. provider input decision;
8. provider send lease acquisition;
9. provider attempt preparation;
10. external IO fence recording;
11. fenced provider send;
12. provider output staging and evidence observation;
13. tool-barrier input recording;
14. batch commit plan sealing;
15. transcript publication;
16. batch publish outcome;
17. terminalization.

The machine MUST own:

- operation phase;
- lifecycle terminal meaning;
- retry refusal;
- source and target denial;
- barrier membership and ordering;
- commit plan semantics;
- publish success or failure.

Provider crates MUST produce request-preparation and provider-output facts with
no embedded operation terminal class.

## 9. Provider Request Preparation and Execution

The prepared request envelope MUST contain:

- prompt;
- resolved target provider/model/backend;
- machine-resolved source identities;
- machine-resolved reference identities;
- hydration evidence;
- accepted output contract;
- accepted provider parameter evidence.

The prepared request artifact MUST be self-contained for selected image sources:
it contains selected source bytes. Hosted provider ids, previous response ids,
ambient provider context, temp files, mutable blob refs, durable indirection
refs, and builder handles MUST NOT be source or recovery authority.

Only a machine-accepted prepared artifact MAY be sent.

OpenAI MAY use Responses hosted image tooling or Images API backends. The
selected backend MUST be recorded in the target-resolution decision. OpenAI
request builders MUST serialize only that selected backend request.

Gemini MAY use native image-capable Gemini models. Gemini image generation MUST
be operation-local in v1. It MUST NOT mutate the session model, apply a scoped
session model override, or run a restore lifecycle.

## 10. Commit and Adjacency Contract

Generated images MUST be committed through the assistant tool batch.

The transcript order for one successful `generate_image` call MUST be:

1. assistant message containing the tool use;
2. tool-results message for the batch;
3. assistant message containing generated `ContentBlock::Image` output.

Generated image content MUST NOT appear between a tool call and its tool result.

One batch MUST publish one `ToolResultsMessage` followed by one assistant image
message per successful `generate_image` call, in tool-call order. Each assistant
image message MUST preserve provider output order.

`BatchCommitPlan` MUST be the only transcript publish plan. It MUST seal
ordering, message ids, block ids, timestamps, image publish entries, reserved
refs, and expected digests before shell publication.

Tool success for `generate_image` is complete only when the linked assistant
image content is published in the same transcript commit receipt.

## 11. Recovery Contract

Recovery MUST use durable machine/evidence boundaries:

- image operation ledger;
- transcript boundary and source-token manifest;
- prepared provider attempt;
- external IO fence;
- provider output receipt;
- batch commit plan;
- transcript commit receipt.

Missing machine-owned manifest before preparation MUST terminalize as typed
invariant recovery failure. Pre-Prepared recovery MUST terminalize typed
preparation-interrupted/no-send and MUST NOT call the provider.

Prepared-without-fence recovery MAY resume the normal send path only after
AuthMachine confirms same-binding renewal. AuthMachine typed denial MUST
terminalize typed failure. Recovery MUST NOT change endpoint, body, provider,
policy, target, source selection, or provider params.

Before `ExternalIoFence`, cancellation, timeout, or local failure for an admitted
operation MUST record typed no-send barrier input. After the machine enters
send-fenced phase, transport timeout, process crash, cancellation, or local
failure before known provider response MUST terminalize as unknown send outcome.

Fenced-without-output recovery MUST NOT reissue the provider call. It MUST
terminalize as unknown send outcome with typed cause evidence.

Provider output without transcript publication MUST resume publication from the
sealed `BatchCommitPlan`. It MUST NOT call the provider again.

Provider output without barrier input or plan MUST feed each member's durable
evidence through the existing tool-batch transitions. No sibling may be
re-executed unless its own state proves no external send occurred.

Provider bytes observed but not durably staged MUST terminalize as typed
output-staging failure, not unknown send.

For missing transcript receipts, the machine MUST emit
`LookupTranscriptCommitByReservation`. A matching receipt means recovered.
Durable rejection or digest mismatch means `BatchPublishFailure`. Otherwise
publish remains pending.

V1 MUST NOT use provider-specific retry or resume protocols.

## 12. Public Surface and SDK Contract

Public surfaces and SDKs MUST expose:

- sending user image content;
- reading assistant image content;
- reading tool-result image content;
- blob retrieval for display;
- turn/batch status derived from machine publish outcome;
- convenience helpers derived from canonical transcript content.

Public surfaces and SDKs MUST NOT expose:

- direct image-generation methods;
- source-token construction;
- surface image handles;
- provider execution knobs outside normal session/model configuration;
- alternate transcript schemas.

SDK documentation SHOULD explain how to provide image input to the assistant and
how to read generated image output. It MUST NOT document `generate_image` as an
SDK method.

## 13. Implementation Slices

Each slice MUST leave the workspace buildable. Any slice that changes public
transcript shape MUST update Rust, wire schemas, SDK types, surfaces, and tests
in the same slice.

### 13.1 Canonical Transcript Content

- Remove canonical `Message::BlockAssistant`.
- Remove `AssistantBlock::Image`.
- Represent visible assistant media as `AssistantBlock::Content`.
- Add `created_at` to every canonical transcript message.
- Cut over wire schema, persistence, surfaces, SDKs, and tests.
- Reject old rows as read errors.
- Prove enabled `generate_image` never uses generic tool failure.

### 13.2 Structural Media Traversal

- Make hydration, externalization, reachability, compaction, indexing, and hooks
  walk `ContentBlock` structurally.
- Cover user, assistant, and tool-result images.
- Ensure tool-result image blocks have stable `BlockId`s.
- Keep provider request construction disabled.

### 13.3 Machine Image Operation Protocol

- Add catalog phases, terminals, effects, and receipts.
- Add assistant-only admission.
- Add target resolution, source resolution, hydration evidence, prepared
  attempts, send fences, barrier inputs, batch commit plans, and publish
  outcomes.

### 13.4 OpenAI and Gemini Providers

- Move image model ownership and provider params into provider crates.
- Implement provider profile facts, request builders, and response parsing.
- Keep Gemini execution operation-local.
- Prove semantic denial is not first discovered in request builders.

### 13.5 Docs, Skills, and Live Smokes

- Update README, docs, and `.claude/skills/meerkat-*`.
- Document assistant-only image generation.
- Add cross-provider live smokes with explicit selected images.

## 14. Conformance Tests

An implementation conforms when:

- canonical transcript serde accepts only the new assistant shape;
- every transcript message carries `created_at`;
- user, assistant, and tool-result images hydrate structurally;
- `generate_image` is unavailable from public surfaces;
- non-assistant `generate_image` attempts fail typed machine denial;
- assistant `generate_image` can select user, assistant, and tool-result images;
- prompt-string source inference is absent;
- compacted-away images are absent from later manifests;
- selected source bytes seal into prepared artifacts;
- OpenAI and Gemini receive selected source/reference bytes;
- provider-specific params produce typed evidence;
- request builders serialize only approved requests;
- generated images publish after tool results;
- tool-call/tool-result adjacency is preserved;
- publish failure is represented by machine outcome, not inferred from rows;
- recovery covers prepared-without-fence, fenced-without-output, output without
  transcript commit, missing receipt, recovered receipt, digest mismatch, and
  final publish failure;
- Python and TypeScript SDKs can send user images and read assistant images;
- live smokes prove selected blob hashes flow into provider requests.
