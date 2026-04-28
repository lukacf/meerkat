# ADR: Assistant Image Generation Substrate

**Status**: Draft rewrite
**Date**: 2026-04-27
**Legacy reference**:
`docs/architecture/assistant-image-generation-substrate.legacy-2026-04-27.md`

## Purpose

Defines assistant image generation.

- `generate_image` is assistant-only.
- It is not a user-callable CLI, SDK, REST, RPC, MCP, or surface API.
- Images enter the system as canonical transcript content.
- OpenAI and Gemini are the v1 provider targets.

Not a universal media projection platform. Dogma:

- one semantic fact, one owner;
- machines own lifecycle;
- shell owns IO mechanics;
- provider crates own provider-specific models, params, and wire shapes;
- surfaces are skins, not authorities;
- projections are rebuildable, never source truth.

## Scope

- invocation;
- explicit provider/model target or provider default;
- output intent;
- edit sources and references;
- image sources;
- OpenAI/Gemini;
- assistant images;

Unsupported:

- user-callable tool;
- surface handles;
- anchors after issuing invocation;
- background.

## Core Decisions

### 1. `generate_image` Is Assistant-Only

`generate_image` is a tool-shaped `MeerkatMachine` operation. Application
callers never invoke it directly.

Surfaces may:

- send user messages with image content;
- render assistant messages with generated image content;
- fetch blob bytes for displayed images;
- expose SDK helpers for reading image blocks.

Surfaces must not call `generate_image`, pass handles, authorize sources, or decide provider/model semantics.
For user image ingress, surfaces submit bytes/hints. Shell records byte, media,
digest, and dimension evidence; `MeerkatMachine` transcript admission admits
media type and allocates ids, visibility, and commit truth.

Users provide image input as session content. The assistant decides whether to
call `generate_image`.

Tool visibility is machine-owned. `PrepareAssistantInvocation` records
`AssistantInvocationOffer { manifest_id, tools }`.
Model-visible source offers render directly from the sealed manifest.
Manifest mint failure terminalizes `PrepareAssistantInvocationFailed`; recovery
does not invoke the model.
Non-assistant attempts fail typed
`NotAssistantToolBatch`.

### 2. Transcript Content Is The Only Image Source Authority

All image inputs to `generate_image` resolve from canonical transcript identity.

Eligible sources are:

- user message image blocks;
- assistant image blocks;
- tool-result image blocks.

The source resolver accepts explicit machine source tokens only:

```rust
pub struct SourceToken {
    pub token_id: SourceTokenId,
}
```

`BlockId` is a transcript-owned content id. The invocation `SourceTokenManifest`
is durable token evidence for one invocation boundary. It cites transcript commit
receipts and token -> content id. The assistant must choose an exact visible
image block. V1 has no shorthand refs. Labels are hints, never authority.
Tokens are looked up only in the active invocation manifest.

`TranscriptBoundaryId` is machine-minted for one model invocation snapshot. It
names the exact visible transcript read boundary after compaction, not the whole
session forever. Later snapshots do not mint refs for compacted-away blocks. The
machine records the boundary and `SourceTokenManifest` before model invocation.
The manifest is a token snapshot: active-operation eligibility
cites transcript commit truth. Later invocations mint fresh tokens for visible prior blocks;
old tokens are rejected outside their manifest.
Manifest owns token-offer eligibility only; transcript receipts own content facts.

The model-facing token is manifest-scoped and opaque. Helpers and surfaces cannot
parse it or keep a registry. If tokens cannot be emitted, the machine fails
before model invocation. Unknown tokens are rejected. Tokens are valid only for
tool calls from that invocation, through its batch admission window, never later.
Transcript retention keeps offered blobs reachable until admission closes;
selected source retention extends through hydration and prepared-artifact sealing.

The runtime must not infer sources from prompt strings such as "the previous
image", "that screenshot", or "make it brighter". The assistant may interpret
the user's natural-language request and choose an explicit transcript reference
in the tool call; the machine resolves only that reference.

Invalid transcript refs, compacted-away blocks, invisible blocks, non-image
blocks, and non-blob image data fail through typed machine-owned
source-resolution denial. Missing, unreachable, or digest-mismatched blob bytes
fail later through typed hydration failure. There is no text-only fallback.

### 3. Generated Images Are Assistant Content

Generated images are visible assistant content:

```rust
pub struct UserMessage {
    pub message_id: MessageId,
    pub created_at: Timestamp,
    pub blocks: Vec<ContentBlockEntry>,
    pub meta: UserMessageMeta,
}

pub struct AssistantMessage {
    pub message_id: MessageId,
    pub created_at: Timestamp,
    pub blocks: Vec<AssistantBlock>,
    pub meta: AssistantMessageMeta,
}

pub enum AssistantBlock {
    Content(ContentBlockEntry),
    ToolUse {
        block_id: BlockId,
        id: ToolCallId,
        name: ToolName,
        args: Box<RawValue>,
        replay: ProviderReplayRef,
    },
    Reasoning {
        block_id: BlockId,
        replay: ProviderReplayRef,
    },
}

pub enum ContentBlock {
    Text { text: String },
    Image { media_type: MediaType, data: ImageData },
}

pub enum ImageData {
    Blob { blob_ref: ContentBlobRef },
}

pub struct ContentBlockEntry {
    pub block_id: BlockId,
    pub block: ContentBlock,
}

```

All transcript messages carry `message_id` and `created_at: Timestamp`.
The machine allocates timestamps from shell clock evidence as display metadata,
not ordering/lifecycle truth.

There is no `Message::BlockAssistant`. There is no
`AssistantBlock::Image`. Visible media uses `ContentBlock::Image` regardless of
whether it came from a user, a tool result, or assistant image generation.

Provider metadata, revised prompts, ids, warnings, and traces are audit facts,
not visible content. Audit views are rebuildable projections.
Generated-image provenance lives in commit evidence, not visible content.

`MeerkatMachine` owns transcript reservations. `BatchCommitPlan` seals them:
ids/order, `ReservedContentRef`s, and visibility intent are recorded before shell
commit. Receipt publishes `ContentBlobRef`s; registries rebuild from receipts. Raw `BlobId`
values are shell mechanics, never visible handles or source refs.

### 4. Tool-Result Images Are Transcript Content

Tool results may contain content blocks:

```rust
pub struct ToolResultsMessage {
    pub message_id: MessageId,
    pub created_at: Timestamp,
    pub results: Vec<ToolResult>,
}

pub struct ToolResult {
    pub tool_call_id: ToolCallId,
    pub status: ToolResultStatus,
    pub blocks: Vec<ContentBlockEntry>,
}

```

A tool-result image block is eligible only when the current invocation manifest
re-offers that prior transcript block as a fresh `SourceToken`. Prior tokens
never survive as authority. This supports workflows such as:

1. assistant calls a screenshot tool;
2. the screenshot tool returns an image block;
3. a later assistant invocation calls `generate_image` using that image;
4. assistant outputs the edited image.

This does not require surface handles because the image already exists in the
canonical transcript.

Tool-result `message_id`, block ids, and `ReservedContentRef`s are reserved in
`BatchCommitPlan`. `TranscriptCommitReceipt` publishes `ContentBlobRef`s.

### 5. Provider-Specific Parameters Stay In Provider Crates

The model-facing request has universal fields plus an opaque provider parameter
payload. Opaque means "opaque to core and runtime," not "untyped forever":

```rust
pub struct GenerateImageRequest {
    pub prompt: String,
    pub kind: ImageOperationKind,
    pub target: ImageTargetIntent,
    pub provider_params: ProviderParamsIntent,
    pub output: ImageOutputIntent,
    pub source_images: Vec<SourceToken>,
    pub reference_images: Vec<SourceToken>,
}

pub enum ImageTargetIntent {
    Explicit { provider: ProviderId, model: ModelId },
    ProviderDefault { provider: ProviderId },
}

pub enum ProviderParamsIntent {
    ProfileDefaultsOnly,
    UnnormalizedCustom(Box<RawValue>),
}

pub struct ImageOutputIntent {
    pub count: NonZeroU32,
    pub format: ImageFormatIntent,
    pub size: ImageSizeIntent,
    pub quality: ImageQualityIntent,
}
```

`ImageOutputIntent` has no silent defaults.
`MeerkatMachine` owns provider-agnostic count, format, size intent, and quality: `png|jpeg|webp`,
size `auto|square|portrait|landscape`, quality `auto|low|medium|high`.
Provider profiles own capability/mapping facts and native meanings.

Provider params exist separately. Profile defaults are non-disableable v1
facts; `ProfileDefaultsOnly` means no custom params. Params cannot affect target/backend selection. Schemas come from
`ModelProfile`; raw JSON is ingress only. Provider normalization returns
`ProviderParamsEvidence { schema_revision, digest, effective_values, origins,
conflicts }`. Schemas reject universal-output duplicates. Builders serialize only accepted values.
Params normalize after target resolution; no provider-input decision occurs until typed.

Providers own `ModelProfile` facts:
- supported image model names;
- provider-local default model;
- backend identities/defaults;
- backend candidate facts;
- parameter docs/shapes;
- request serialization and response parsing.

Provider crates own profile facts; the factory seam supplies typed reads.
`MeerkatMachine` records `ProviderProfileSnapshot { revision, digest }`.
Profile changes do not affect admitted operations; new ones read fresh snapshots.
`MeerkatMachine` records backend selection inside `ImageTargetResolutionDecision`
by backend candidate facts, target policy, and auth.
Builders never choose or fallback.

### 6. Target Resolution Is Machine-Owned

The assistant supplies `ImageTargetIntent`; `MeerkatMachine` owns the
canonical target. Helper code is pure mechanics and cannot own target facts.
Target resolution consumes:

- the assistant target preference;
- `ProviderProfileSnapshot` from provider crates;
- `SessionPolicySnapshot` from `MeerkatMachine` image policy.

`ImageTargetIntent` is `Explicit { provider, model }` or
`ProviderDefault { provider }`; provider identity is never
inferred from model names. No cross-provider auto target. Machine table: explicit -> provider
model/backend decision -> target/auth policy admission; provider default ->
`ProviderDefaultModelEvidence { model, backend, profile_revision }` -> target/auth
admission. `ModelProfile` owns that default fact; `MeerkatMachine` admits or
denies it.
Provider input observations are emitted after target resolution.
The factory/facade composes typed image policy overrides; `MeerkatMachine`
snapshots/applies them and records snapshot id/revision/digest.
`AuthMachine` owns identity binding facts/revision; shell credential probes are
evidence only. The seam is typed: MeerkatMachine requests lease evidence/renewal
and owns operation terminalization from AuthMachine's response. Provider, model,
session, owner, policy, or auth changes require fresh policy snapshot.

`ImageTargetResolutionDecision` is the sole target/backend decision.
`ModelProfile` owns provider-local facts, not selection or policy.

## Machine-Owned Lifecycle

The image operation lifecycle belongs to the catalog `MeerkatMachine`. The shell
executes IO and supplies evidence; it does not decide lifecycle truth.
This extends the existing machine DSL with keyed fields/effects, not a helper
authority or new top-level machine.

The v1 lifecycle is:

1. assistant calls `generate_image` inside the active assistant tool batch;
2. machine admission accepts the call and begins an image operation for that
   batch, bound to the invocation manifest id;
3. machine validates universal operation shape;
4. machine resolves provider/model/backend from profile facts;
5. machine records
   `ImageTargetResolutionDecision`;
6. provider/profile code normalizes params; machine records typed param evidence;
7. machine resolves explicit source/reference images from transcript identity;
8. machine emits `HydrateImageSources`;
9. shell hydrates selected blob-backed image bytes and records
   `ImageHydrationEvidence`;
10. provider/profile code emits backend-specific facts and request DTO candidate;
11. machine records `ProviderInputDecision::Denied` as failure-shaped
   `ToolBarrierInput`, or `ProviderInputDecision::Accepted` and emits
   `AcquireProviderSendLease`, then `PrepareProviderAttempt` with attempt id, nonce, and
   correlation id;
12. shell materializes the durable prepared artifact;
13. machine accepts artifact evidence and writes `ProviderAttemptRecord::Prepared`;
14. machine emits `RecordExternalIoFence` for that prepared attempt;
15. shell durably records `ExternalIoFence`;
16. machine observes the fence and emits `PerformFencedProviderSend`;
17. shell sends the recorded provider request;
18. shell stages and finalizes provider output bytes or records typed staging
    evidence;
19. machine observes output/storage evidence and records `ToolBarrierInput`;
20. after every barrier member has barrier input, `MeerkatMachine` records
    sealed `BatchCommitPlan`;
21. tool-batch commit publishes tool results and assistant image content
    atomically within the session transcript;
22. machine records `BatchPublishOutcome`; only published success is final image
    operation success.

The machine owns:

- operation phase;
- assistant-only admission;
- target-resolution decisions;
- source-resolution decisions;
- hydration evidence interpretation;
- provider-attempt preparation and send-fenced phase;
- provider-call attempt observation;
- execution failure outcome from canonical provider output evidence;
- semantic storage outcome;
- tool-batch barrier membership and ordering;
- tool-batch commit outcome;
- terminal outcome.

The shell owns:

- blob reads/writes;
- provider HTTP/API calls;
- durable attempt/output/publish evidence;
- byte/media inspection evidence;
- storage evidence and receipts.

Provider crates own:

- provider request fact production;
- request DTOs, wire encoding, and response parsing;
- mapping provider wire facts into factual output evidence.

Provider crates produce typed request-preparation and provider-output facts, with
no embedded operation terminal class. `MeerkatMachine` alone maps those facts to
operation phase, denial, retry refusal, and terminalization.

## Source Resolution

Source resolution is machine-owned and transcript-only.

Input:

```rust
pub struct ImageSourceResolutionRequest {
    pub operation_id: ImageOperationId,
    pub manifest_id: SourceTokenManifestId,
    pub source_images: Vec<SourceToken>,
    pub reference_images: Vec<SourceToken>,
}
```

The manifest is the active operation's transcript-bound eligibility proof.
Source resolution never re-queries current transcript/projections.

Output:

```rust
pub enum ImageSourceResolutionOutcome {
    Resolved {
        decision_id: MachineDecisionId,
        sources: Vec<ResolvedImageSource>,
        references: Vec<ResolvedImageSource>,
    },
    Denied {
        decision_id: MachineDecisionId,
        reason: ImageSourceResolutionDenial,
    },
}
```

The resolver checks manifest facts only:

- the manifest contains the referenced message/block at the transcript boundary;
- the block is visible;
- the block contains `ContentBlock::Image`;
- the image content names blob-backed transcript data.

The resolver does not:

- inspect prompt strings;
- consult provider adapters;
- accept raw `BlobId`;
- accept surface-local handles;
- read blob storage;
- decide blob reachability;
- resurrect compacted-away images.

Compaction may summarize an image in text, but that summary is not an image
source. Removed image blocks are not selectable in new invocations.

Blob reachability is not source-resolution truth. After the machine resolves
transcript eligibility, the shell hydrates the selected blob references and
records `ImageHydrationEvidence` with media type, byte digest, dimension state
`Measured | Unavailable { cause } | NotRequired`, and byte availability/cause.
The machine owns the terminal meaning of that evidence.

## Provider Request Preparation

V1 prepares one concrete provider request. It does not validate the whole
transcript.

The concrete request envelope contains:

- prompt;
- resolved target provider/model/backend;
- machine-resolved source image identities;
- machine-resolved reference image identities;
- shell-supplied source image hydration evidence;
- shell-supplied reference image hydration evidence;
- machine-accepted output contract;
- accepted `ProviderParamsEvidence`.

The machine validates universal shape and role matrix before source resolution.
`source_images` are edit bases: `Edit` requires them and `Generate` rejects them.
`reference_images` are non-edited guidance images selected through the same
manifest. Duplicate tokens and cross-role reuse are typed machine denial.
Provider/profile code emits input observations:

- parameter payload shape;
- source/reference count and capability;
- media type and size;
- output size/aspect/quality/format support.

Target resolution owns provider/model/backend/policy/auth only. Provider crates
emit typed compatibility facts; `ProviderInputDecision` records/terminalizes them.

Machine output acceptance requires exactly requested count valid images. Fewer,
extra, or mixed valid/invalid images terminalize typed provider-output failure;
warnings attach as audit evidence.

Provider/profile code produces request DTO candidate. Machine accepts
`ProviderRequestFact { target_decision_id, source_decision_id, hydration_id,
output_contract_id, params_evidence, dto_digest }`; shell materializes artifact
bytes whose digest must match. Credentials are excluded; `Prepared` records
AuthMachine lease revision separately. No transcript refs, mutable blob refs,
temp files, or builder handles are allowed. Only `Prepared` is sendable or
recoverable. Recovery never rehydrates from current transcript, policy,
provider registry, or blob refs. Shell reports artifact facts; the machine
compares them to accepted revisions.

`RecordExternalIoFence` is legal only for a prepared attempt. `PerformFencedProviderSend`
is legal only after the machine observes durable fence evidence and enters
send-fenced phase. Before that phase, local failure is typed no-send; after it,
missing provider output is unknown send. Request builders serialize only the
prepared artifact. Builder failures are mechanical/internal defects.

## Provider Execution

OpenAI support may use Responses hosted image tooling or Images API paths.
`MeerkatMachine` records the selected backend in `ImageTargetResolutionDecision`.
OpenAI v1 profile supplies backend candidate facts.
Provider code builds the typed request DTO for that backend. No backend fallback
occurs in v1. Request builders only serialize that DTO.
Provider-private backend outcomes map into canonical provider-output evidence.
Hosted ids are audit/mechanics, never hidden source/recovery truth. The prepared
request artifact is self-contained for selected image sources: it contains source
bytes. No hosted file id, previous response id, or ambient provider context may
be the source or recovery authority.

Gemini support may use a Gemini native image-capable model, but the target is
operation-local. V1 does not mutate the session model, apply a scoped model
override, or run a restore lifecycle for image generation. If Gemini needs
provider-native context, that context is represented inside the approved
operation-local provider request artifact, not as session model reconfiguration
or hidden transcript replay state.

Provider output evidence is a canonical machine-protocol enum. Provider crates
only map wire facts into it and attach audit metadata. The machine owns
terminalization.

## Commit Shape And Tool Adjacency

Generated image output is committed through the assistant tool batch.

The transcript shape is:

```text
Assistant(tool_use generate_image)
ToolResults(success, blocks=[])
Assistant(ContentBlock::Image ... for one generate_image call)
```

Generated image content never sits between tool call and result.
Tool success means provider output accepted and image message committed in batch;
the empty tool result alone is not visible image content.
Generated image messages are machine-authored by `BatchCommitPlan`, not provider
replay units. `ImagePublishEntry { operation_id, tool_call_id, output_refs,
assistant_message_id, block_ids }` lives in the plan; receipt proves publish.
Commit evidence, not visible empty tool status, is authoritative for success.
A successful `generate_image` result is complete only when its linked assistant
image ids are exposed from the same `TranscriptCommitReceipt`.
Stale projections never gate behavior.

Provider output bytes become durable blob evidence before transcript publication.
Only transcript commit publishes selectable `ContentBlobRef`s.

The session transaction publishes one `ToolResultsMessage` for the batch, then
one assistant image message per successful `generate_image` call, in tool-call
order; each message stores output image blocks in provider output order.

It atomically publishes the tool result receipt and assistant image messages.

Either all session-visible rows publish with one transcript allocation, or none
do. If finalized bytes outlive a failed transcript commit, they are orphaned
mechanical storage leased by the machine until `BatchPublishOutcome` is terminal.
GC acts only after terminal state.

Non-image sibling tool results share the same tool-result message. Before
`BatchCommitPlan`, image failure contributes a failure-shaped result and does
not block sibling barrier inputs.

The existing machine-owned assistant tool-batch protocol owns membership,
readiness, and order; image generation contributes typed barrier inputs only.
`ToolBarrierInput` is only a machine-consumed evidence packet.
`MeerkatMachine` records `execution_outcome` separately for published tool results.
Multiple `generate_image` calls commit image messages in tool-call order, each
preserving provider output order. Pre-plan sibling failures do not reorder
successful sibling outputs.

The image audit view is rebuilt from the operation ledger, provider preparation,
provider output, storage, and transcript commit receipts. It may be
dropped and rebuilt on demand or after receipt writes; stale audit state never
gates behavior.

After all barrier members have barrier input and before transcript
publication, `MeerkatMachine` records sealed `BatchCommitPlan`.
That plan is the sole transcript publish plan: ordering, message/block ids,
`created_at` values, image publish entries, reserved refs, and expected digests. Provider
output receipts are cited input evidence, not content
authority.
`ReservedContentRef` is never source-resolvable; receipt publishes `ContentBlobRef`.
Raw storage identity never publishes content.

Barrier input and batch publish outcome are different. Operations stay
non-terminal after `BatchCommitPlan`; `Published` finalizes success and
`BatchPublishFailure` terminalizes plan members. Before
`BatchCommitPlan`, image failure contributes a failure-shaped tool result and
successful siblings remain eligible. After `BatchCommitPlan`, shell reports raw
storage facts: reserved ids absent, commit rejected, receipt missing, or digest
mismatch. For missing receipts, machine emits `LookupTranscriptCommitByReservation`;
shell returns durable facts. Matching receipt means recovered; durable rejection
or digest mismatch means `BatchPublishFailure`; otherwise publish remains pending.
`MeerkatMachine` derives one `ImageOperationTerminal` from execution and
publication evidence: `Published`, `FailedPublished`, `PublishFailed`, or
`UnknownSend`. `PublishFailed` preserves unpublished execution outcome.
Surfaces render machine turn status for unpublished publish failure, never infer from rows.
`BatchPublishOutcome` carries per-operation terminals.
Post-fence no output terminalizes `unknown-send`; recovered publish -> `Published`.

## Recovery

V1 keeps recovery truthful.

Durable recovery boundaries are:

- image operation ledger: machine decisions and terminal meaning;
- transcript boundary and visible image ref manifest;
- `ProviderAttemptRecord::Prepared`: machine-accepted request before external IO;
- send-fenced machine phase: provider send outcome is unknown;
- `ProviderOutputReceipt`: recoverable output evidence;
- `BatchCommitPlan`: sealed publish plan for the batch;
- `TranscriptCommitReceipt`: published tool result and assistant image content.

`ProviderAttemptRecord::Prepared` owns attempt identity, accepted revisions,
prepared send-envelope digest, artifact ref, and `AuthLeaseEvidenceRef`.
Only AuthMachine interprets binding validity, expiry, and renewal.
AuthMachine owns lease validity, renewal, and denial. Prepared-without-fence
recovery asks AuthMachine for `RenewedSameBinding` or typed denial for that
digest; it cannot change endpoint, body, provider, policy, or target.
The machine consumes that fact, then sends or terminalizes typed failure.
Absence of send-fenced phase is canonical no-send. Presence means send outcome is
unknown. Shell `ExternalIoFence` is mechanical input to that phase, not the phase
itself. Recovery feeds any durable fence evidence through the machine transition
before classifying prepared/fenced. Missing machine-owned manifest is typed invariant
recovery failure before `Prepared`; later recovery uses the later durable boundary.
Pre-Prepared recovery terminalizes typed preparation-interrupted/no-send. It
never replays target/prep semantics or calls provider.

`ProviderOutputReceipt` must contain recoverable output evidence, not just
metadata. Successful image output requires durable staged bytes plus digest,
media type, typed dimension evidence, and provider output index. Typed
provider-output facts without bytes are denial/failure evidence only and cannot
contribute to the requested image count. If provider bytes were observed but
cannot be durably staged, the machine records typed output-staging failure, not
unknown send outcome.

If recovery finds prepared-without-fence, it replays the normal machine send
path. Fenced-without-output is not reissued and terminalizes typed
unknown-send-outcome. The user may ask again.

Before `ExternalIoFence`, cancellation, timeout, or local failure for an admitted
operation records typed no-send barrier input. After `BatchCommitPlan`, they
follow publish recovery/failure rules.

After send-fenced phase, transport timeout, process crash, cancellation, or local
failure before provider response is known terminalizes as `unknown-send-outcome`
with typed cause evidence.

With provider output and plan but no final transcript commit, recovery resumes
publish from that plan. Finalized blob receipts are reused; unreachable or
mismatched orphan blobs make `PublishPending` unrecoverable and record
final `BatchPublishFailure`. It does not call the provider again.

With provider output but no barrier input/plan, recovery feeds each member's
durable evidence through existing tool-batch transitions. No sibling is
re-executed unless its own state proves no external send occurred.

This avoids provider-specific retry/resume protocols in v1.

## Public Surfaces And SDKs

No public surface exposes `generate_image` as a callable API.

Surfaces and SDKs expose:

- sending user messages with `ContentBlock::Image`;
- reading assistant messages with `ContentBlock::Image`;
- reading tool-result messages with image blocks;
- blob retrieval for image display;
- per-operation turn/batch status from `BatchPublishOutcome`;
- convenience helpers derived from canonical transcript content.

They do not expose:

- direct image-generation methods;
- surface image handles;
- provider execution knobs outside normal model/session configuration;
- alternate transcript schemas.

SDK docs should explain how to provide image input to the assistant and how to
read generated image output. They should not document `generate_image` as an SDK
method.

## Implementation Plan

Each slice must leave the workspace buildable. A slice that changes public
transcript shape must update Rust, wire schemas, SDK types, surfaces, and tests
in the same slice.

### Slice 1: Canonical Transcript Content

- Replace `Message::BlockAssistant` and `AssistantBlock::Image` with canonical
  assistant `ContentBlock::Image`.
- Ensure user, assistant, and tool-result visible media all use `ContentBlock`.
- Add `created_at` to every canonical transcript message. Core constructors
  stamp messages from `MessageTimestamp::now`; tests and recovery paths may pass
  explicit values. Wire schema, persistence, REST/RPC/MCP history, CLI
  rendering, Python SDK, and TypeScript SDK must carry the field. If a
  transitional row still exists, timestamp it too, then delete it before
  acceptance.
- Legacy rows are read errors, never source-visible; storage reports evidence.
- Cut over all surfaces and SDKs to the canonical shape.
- Keep `generate_image` unavailable until the machine protocol is live; generic
  tool failure must not survive the enabled feature.

Gate:

- `make check`;
- core/wire serde fixtures;
- SDK parser tests;
- surface history tests;
- negative tests for old assistant rows;
- gate proving enabled `generate_image` never uses generic tool failure.

### Slice 2: Structural Media Traversal

- Make hydration, externalization, reachability, compaction, indexing, and hooks walk `ContentBlock`.
- Cover user, assistant, and tool-result images.
- Ensure tool-result image blocks have stable `BlockId`s.
- No provider request construction yet.

Gate:

- tests that all transcript image roles externalize, hydrate, and collect blob ids;

### Slice 3: Machine Image Operation Protocol

- Add catalog machine phases and terminal classes for the v1 lifecycle.
- Add assistant-only tool-batch admission with typed denial for non-assistant
  attempts.
- Add target-resolution evidence with typed denial for unsupported provider,
  model, policy, or unavailable identity.
- Add machine-owned source resolution for explicit transcript refs.
- Add hydration evidence observation and typed hydration failures.
- Add provider-attempt preparation and send-fence recovery.
- Add tool-batch commit protocol for generated assistant image content.
- Add durable `BatchCommitPlan`.
- Add screenshot/tool-result image source tests.
- Add multiple-image-operation batch ordering tests.

Gate:

- machine codegen;
- machine verification;
- drift/parity checks;
- focused adjacency and commit atomicity tests;
- assistant-only admission denial tests;
- target-resolution denial tests;
- recovery tests for prepared-without-fence and fenced-without-output;
- recovery tests for output-without-transcript;
- fake-provider tests proving selected tool-result bytes reach providers.

### Slice 4: OpenAI And Gemini Providers

- Move image model ownership and provider params preparation into OpenAI/Gemini.
- Implement provider-profile preparation of the concrete image request.
- Implement OpenAI and Gemini request builders and response parsing.
- Implement Gemini as operation-local image target execution, without scoped
  session model override/restore.

Gate:

- `ModelProfile` tests;
- provider request-builder tests for approved requests;
- request-builder mechanical-failure tests;
- tests proving selected tool-result image bytes are sent;
- provider-evidence to machine-denial mapping tests;
- typed provider-param parsing evidence tests;
- no semantic denial first discovered in request builders.

### Slice 5: Docs, Skills, And Live Smokes

- Update README, docs, and `.claude/skills/meerkat-*`.
- Document `generate_image` as assistant-only.
- Document SDK image input/output handling, not direct image generation.
- Add cross-provider smoke using explicit selected images.

Gate:

- Python/TS SDK tests;
- live smoke when provider keys are configured.

## Invariants

- `generate_image` is assistant-only.
- Image sources resolve from transcript identity only.
- All visible images use `ContentBlock::Image`.
- No surface-local image handle exists.
- No prompt-string source inference exists.
- No implicit passive image history is sent.
- Compacted-away images are not selectable.
- Provider-specific model and params truth lives in provider crates.
- Request builders serialize approved requests.
- Provider calls happen only from prepared attempts.
- Provider send outcomes are unknown only after machine enters send-fenced phase.
- Generated assistant image content is committed after tool results.
- Unknown send outcome is terminal.

## Acceptance Tests

The feature is done when:

- canonical transcript serde accepts only the new assistant shape;
- all image-bearing transcript roles hydrate structurally;
- `generate_image` is unavailable from public surfaces;
- assistant-only `generate_image` can use a user image source;
- assistant-only `generate_image` can use an assistant image source;
- assistant-only `generate_image` can use a tool-result image source;
- OpenAI receives selected source/reference image bytes;
- Gemini receives selected source/reference image bytes;
- generated assistant images are committed after tool results;
- invocation tokens survive compaction through batch admission;
- selected source bytes seal into prepared artifacts;
- newly compacted-away images are absent from later manifests;
- provider-specific params produce typed fact evidence and machine decisions;
- Python/TypeScript SDKs can send user images and read assistant images;
- post-plan recovery covers missing receipt lookup, recovered receipt, digest
  mismatch, and final `BatchPublishFailure`;
- live smokes prove selected blob hashes flow into provider requests.
