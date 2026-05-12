# Legacy Research Draft: Assistant Image Generation Substrate

**Status**: Revision in progress
**Date**: 2026-04-27

## Purpose

Meerkat needs assistant image generation as a first-class capability without
turning image providers into special application code. The user-facing operation
is simple:

- the active model can ask Meerkat to generate or edit an image;
- the request can target OpenAI, Gemini, or a future image-capable provider;
- provider-specific image parameters remain provider-owned;
- generated images become durable session content;
- later turns can inspect, edit, or reference prior generated images.

The hard part is not calling image APIs. The hard part is preserving Meerkat's
architecture while provider execution shapes differ:

- OpenAI image generation is provider-native behind the active session model.
  The session does not become a `gpt-image-*` chat session.
- Gemini image generation is naturally a native image-output model call. For a
  Gemini image target, Meerkat may run a scoped image-model operation while the
  baseline session model remains unchanged.

This document has two jobs:

1. record the current implemented mechanism;
2. define the target transcript and projection architecture that fixes the
   current media replay gap.

Preserving the interim transcript ontology is not a goal. The clean model is the
goal. This ADR intentionally chooses a clean break: no runtime read
compatibility for old assistant transcript rows.

## Design Dogma

This substrate follows the runtime dogma:

- one semantic fact, one owner;
- machines and generated handoff protocols own lifecycle meaning;
- provider crates own provider wire shapes and provider-specific parameters;
- surfaces are skins, not authorities;
- projections are rebuildable, never authoritative;
- visible media content has one canonical Meerkat representation.

The last point is the correction introduced by this revision:

> Images are `ContentBlock::Image` regardless of whether they came from the
> user, a tool result, or the assistant.

Assistant-specific structure may order content, reasoning, and tool calls, but
it must not define a second image ontology.

## Research Project

This revision is also the research brief for the implementation that should
land after the catalog/production machine parity work. The goal is maximum
confidence before code changes begin, not a loose list of TODOs.

The research project is organized around ten workstreams:

1. **Transcript ontology**: prove the final Rust and wire shapes for
   `UserMessage`, `AssistantMessage`, `ContentBlock`, `AssistantBlock`, and
   their metadata.
2. **Persistence cutover**: decide exactly where old flat assistant messages and
   `block_assistant` messages are rejected. Archival conversion, if ever needed,
   is a separate one-off tool, not part of runtime compatibility.
3. **Generated-image provenance**: decide where `image_id`, operation id,
   dimensions, revised prompt, native metadata, and provider audit evidence
   live after `AssistantBlock::Image` disappears.
4. **Hydration and blob traversal**: define one structural traversal over all
   transcript `ContentBlock`s, independent of role.
5. **Projection selection**: define how a provider call receives selected
   history, explicit source/reference images, compaction decisions, missing-blob
   failures, and replay audit metadata.
6. **Provider lowering**: verify OpenAI, Gemini, Anthropic, compatible OpenAI,
   realtime, and image-native executors lower the same selected projection
   without choosing history themselves.
7. **Tool adjacency**: preserve assistant tool-use / tool-result adjacency while
   still appending generated assistant image content as transcript-visible
   content.
8. **Machine boundary**: after catalog parity lands, keep lifecycle semantics in
   the catalog `MeerkatMachine`; keep provider planning in provider profiles;
   keep provider calls, blob writes, and serialization as shell mechanics.
9. **Surface contract**: update REST, RPC, MCP, CLI, Python SDK, TypeScript SDK,
   schemas, docs, and skills so every surface exposes the same transcript truth.
10. **Live evidence**: use real OpenAI and Gemini image turns to prove image
    continuity comes from replayed bytes/metadata, not from model inference.

### Research questions

The implementation should not start until these questions have crisp answers:

- Is `AssistantBlock::Content { block_id, block, origin }` the final shape, or should
  generated-image provenance be externalized into a transcript audit table keyed
  by `blob_id` and `image_id`?
- Does `AssistantMessageMeta` own `stop_reason`, usage, and provider response
  metadata, or should usage remain entirely session-accounting-owned?
- What is the exact cutover contract for persisted `role: "assistant"` flat
  messages and `role: "block_assistant"` ordered messages?
- Can every provider legally receive selected assistant image content as an
  assistant-role image, or do some providers require role-adapted reference
  projection?
- Which typed tool argument represents "the previous image", how is it resolved,
  and how is that resolution audited?
- What happens when a selected `blob_id` is missing, too large, unsupported by
  the target provider, or stripped by compaction?
- Are Gemini image thought signatures stored in a provider-owned opaque replay
  envelope, and what schema id/version owns that payload?
- Should OpenAI Responses hosted-image turns preserve OpenAI image call IDs or
  previous response IDs as provider replay hints, and if so where?
- Can tool-result image content and assistant image content use the same
  hydration path without breaking function-call adjacency?
- Which breaking wire/SDK changes are required for this branch, and where are old
  rows rejected instead of tolerated?

### Research answers

The ten workstreams produced enough confidence to lock the following decisions:

- **Transcript ontology**: use one canonical assistant message:
  `Message::Assistant(AssistantMessage { blocks, meta })`. Remove canonical
  `Message::BlockAssistant`; old flat `assistant` rows and old
  `block_assistant` rows are not runtime-compatible.
- **Visible content**: use `ContentBlock` for every visible text, image, video,
  or future media item. Replace `AssistantBlock::Text` and
  `AssistantBlock::Image` with `AssistantBlock::Content { block_id, block, origin }`.
- **Metadata**: keep media facts in `ContentBlock`; keep block-origin facts in a
  non-optional `AssistantContentOrigin`; keep provider/output evidence in a
  durable `ImageGenerationAuditRecord` owned by the session store. Machine
  terminal facts remain in machine state/events. Do not put provider response
  IDs, warnings, provider text, or duplicate blob/media facts directly on visible
  content.
- **Usage**: `AssistantMessageMeta` owns `stop_reason`. Usage remains
  session-accounting-owned by default. If per-message usage is needed later, it
  is optional audit metadata, not a billing authority.
- **Cutover**: this is a breaking public wire contract. New history emits only
  canonical `role: "assistant"` rows. `meerkat-core` defines the canonical
  transcript schema and `TranscriptCutoverError`; store/session load and public
  wire decode apply that validator and reject old flat assistant rows,
  `block_assistant` rows, and old `UserMessage.content` aliases.
- **Projection**: `projected_messages: Vec<Message>` is not enough. Use a typed
  `ImageProjectionSnapshot` carrying selected items, omitted items, hydration
  evidence, replay binding refs/digests, policy version, and transcript
  boundary.
- **Image selection intent**: the runtime never infers image replay from prompt
  strings such as "previous image". The model-facing tool exposes typed
  `source_images` and `reference_images`, including selectors such as
  `LatestVisibleAssistantImage`. The active model may infer the user's intent and
  emit those typed tool arguments; Meerkat resolves typed selectors only.
- **Provider lowering**: providers lower selected content only. They may
  role-adapt selected assistant images into legal provider image inputs, but
  they may not scan raw transcript history, choose images, fetch blobs, apply
  compaction policy, or silently drop selected images.
- **Tool adjacency**: the normal tool loop already defers generated image
  transcript effects until after `ToolResults`. The rewrite must preserve that
  invariant and additionally prevent provider serializers from emitting empty
  assistant/model messages for image-only assistant content.
- **Machine boundary**: projection binding and projection-triggered failures are
  not optional shell behavior. Operation phases, projection snapshot binding,
  missing selected media, provider-consumption denial, restoration
  failure, and recovery truth belong in the catalog machine DSL. The projection
  manifest details remain rebuildable audit evidence.
- **Implementation base**: rebase onto the catalog-authoritative branch only
  after checking it has not reintroduced runtime/provider planning folklore.
  Provider-profile planning from the image branch must remain the ownership
  model.

### Evidence gathered

Code inspection shows the current replay gap is concrete:

- `meerkat-core/src/image_content.rs` externalizes and hydrates images in user
  messages and tool results, but not generated assistant image blocks.
- `meerkat-openai/src/client.rs` lowers user images to Responses `input_image`
  data URLs, but skips ordered assistant image blocks.
- `meerkat-gemini/src/client.rs` lowers user images to `inlineData`, but skips
  ordered assistant image blocks.
- `meerkat-anthropic/src/client.rs` lowers user images to Anthropic image
  source blocks, but skips ordered assistant image blocks.
- SDK history parsers and wire schemas currently expose `role:
  "block_assistant"` plus `block_type: "image"` as their generated-image view.

Provider documentation also supports the target direction:

- OpenAI documents multi-turn image generation through the Responses image tool,
  image generation call outputs or image IDs, and input images supplied as URL,
  Base64 data URL, or file ID.
- Gemini documents image editing as text plus image input, recommends
  conversational multi-turn editing, and treats image-generation/editing thought
  signatures as strict replay material for image model parts.
- Anthropic documents vision images as image content blocks supplied by Base64,
  URL, or Files API reference, and notes that multi-turn history resends image
  bytes when Base64 is used.

Live smoke exploration produced the expected failure mode. A first turn created
an image with "Hello"; a follow-up Gemini image prompt that merely asked to add
"Meerkat" produced a new scene instead of a stable edit. That is not definitive
provider proof by itself, but it is exactly the failure expected when generated
assistant image bytes are visible in history but not selected and hydrated into
the next provider request.

### Deliverable 1: Decision table

| Question | Decision | Rationale |
| --- | --- | --- |
| Assistant message shape | One canonical `Message::Assistant(AssistantMessage { blocks, meta })` | A separate `BlockAssistant` role creates two assistant transcript ontologies. |
| User message shape | `UserMessage { blocks: Vec<ContentBlockEntry>, meta: UserMessageMeta }` | User text/images/video already share content semantics; block identity and metadata should not hang as loose fields. |
| Assistant visible content shape | `AssistantBlock::Content { block_id, block: ContentBlock, origin: AssistantContentOrigin }` | Visible assistant text/images/media should use the same representation as user/tool content, while transcript-owned block identity and origin are mandatory and typed. |
| Reasoning shape | Keep `AssistantBlock::Reasoning { block_id, text, replay }` | Reasoning is assistant replay/control structure, not visible content; provider replay material is sidecar-bound, not generic metadata. |
| Tool-call shape | Keep `AssistantBlock::ToolUse { block_id, id, name, args, replay }` | Tool calls are ordered assistant items with replay provenance, not shared provider metadata blobs. |
| Generated image bytes | `ContentBlock::Image { media_type, data: ImageData::Blob { blob_id } }` | Blob/media facts have one owner: visible content. |
| Generated image provenance | `AssistantContentOrigin::GeneratedImage { operation_id, image_id, output_index, replay }` | The visible content block carries a small required origin pointer and replay provenance without duplicating blob facts. |
| Operation/provider evidence | Durable `ImageGenerationAuditRecord` keyed by `ImageOperationId`, stored in session-scoped audit storage | Revised prompts, provider text, response IDs, and warnings are audit facts, not content facts. Machine terminals stay in machine events. |
| Dimensions | Audit evidence unless independently verified from bytes | Provider-reported dimensions are useful but are not byte truth. |
| Stop reason | `AssistantMessageMeta.stop_reason` | This is assistant-message metadata, not a loose top-level field. |
| Usage | Session-accounting-owned by default | Avoid a second billing authority. Optional per-message usage can live under audit metadata later. |
| Generated image turn placement | Separate assistant content message immediately after `ToolResults` | The image is operation output. Keeping it after tool results preserves adjacency and allows surfaces to visually group it with nearby assistant text without merging transcript truth. |
| Legacy rows | No runtime compatibility | Core defines `TranscriptCutoverError`; store/session load and public wire decode apply it and reject old flat `assistant`, old `block_assistant`, and old `UserMessage.content` rows. |

### Deliverable 2: Provider projection matrix

Provider adapters receive an already-selected, already-hydrated projection. The
matrix below describes legal lowering intent; it is not permission for provider
crates to scan raw history.

| Selected transcript item | OpenAI Responses | OpenAI Images API | Gemini | Anthropic |
| --- | --- | --- | --- | --- |
| User text | Lower as text input content. | Lower as prompt/instruction text. | Lower as text part. | Lower as text block. |
| User image | Lower as `input_image` using URL, file id, or data URL where supported. | Lower as edit/source/reference image where the selected backend supports image input. | Lower as `inlineData` or file-backed image part. | Lower as image source block. |
| Assistant text | Lower as assistant text/history item where legal. | Not a transcript replay shape; use only prompt/projection text relevant to the image call. | Lower as model text part where legal. | Lower as assistant text content. |
| Assistant generated image replay | Lower selected canonical image only when the typed approval manifest preserves its `ImageProjectionPurpose`; otherwise typed deny. | Use only `EditSource` / `GenerationReference` images as source/reference inputs. `PassiveHistoryContext` is rejected unless the backend has a distinct passive-context channel. | Lower selected bytes or provider-native continuity reference only under the approved purpose in the approval manifest. | Lower selected bytes only when role/purpose adaptation is approved; otherwise typed reject. |
| Tool result with image | Lower as image-bearing function output where supported; otherwise typed provider limitation, not silent text-only replay. | Not applicable to raw Images API call. | Lower function response plus image parts according to Gemini function/media constraints. | Lower as image-bearing tool result content. |
| Reasoning metadata | Replay only when Responses reasoning adjacency can be preserved unchanged; otherwise omit typed. | Not applicable. | Preserve thought signatures on the same part where provider requires replay. | Preserve thinking/redacted-thinking signatures according to Anthropic adjacency rules. |
| Tool-call metadata | Preserve call IDs and provider function-call metadata. | Not applicable. | Preserve function call/response identifiers and thought signatures on legal parts. | Preserve tool-use IDs and tool-result linkage. |

The key implementation rule is negative: semantic provider-consumption denial is
decided by provider-profile preflight before projection binding. Lowering
serializes the provider plan envelope only after the typed approval manifest has
approved the selected canonical items. A request builder that then discovers it
cannot represent selected media has found an invariant violation, not a second
terminal path. It must not emit an empty assistant/model message and must not
silently degrade the image to text.

### Deliverable 3: Ownership map

| Layer | Owns | Must not own |
| --- | --- | --- |
| `meerkat-core` | Canonical transcript types, `TranscriptCutoverError`, content traversal, blob id reachability, projection data types, opaque provider replay envelope type | Provider model folklore, provider wire JSON, machine lifecycle truth, provider-specific replay enums |
| Catalog `MeerkatMachine` | Image-operation lifecycle, tool-batch commit sub-protocol, typed image-source resolution decisions, complete projection selected/omitted decisions, projection snapshot binding, projection-triggered terminal classes, scoped override lifecycle, semantic retention/visibility/replay-loss decisions, approval/recovery truth, semantic transition guards | Provider request bytes, blob storage mechanics, SDK rendering |
| Runtime shell/factory | Composes provider profiles, invokes machine commands, supplies store/blob observations for source resolution and projection materialization, materializes projections after machine policy decisions, orchestrates provider execution and blob commits | Hardcoded provider model-name semantics, hidden lifecycle decisions, prompt-string intent inference, projection policy ownership |
| Provider crates | Image model profiles, provider-specific parameters, backend selection, request lowering, provider replay envelope codecs, native metadata classification evidence | Transcript selection, blob fetching, durable receipt storage, machine terminalization, surface rendering |
| Store/session layer | Durable session persistence, applying core cutover validation at persisted-session load, history pagination, internal provider replay envelope storage, recovery-critical receipt stores, session-scoped image-generation audit records, projection snapshot rows, commit bundles, and mechanical retention/GC under machine retention decisions | Independent transcript schema semantics, provider lowering semantics, semantic retention/visibility decisions, machine recovery truth, runtime legacy migration |
| Contracts/wire | Public transcript encoding/decoding, applying core cutover validation at public decode boundaries, generated schema emission | Alternate transcript schema semantics, compatibility aliases for removed rows, provider execution semantics |
| Surfaces and SDKs | Rendering, convenience accessors, blob retrieval helpers, examples | Provider execution semantics, projection selection, transcript truth |
| Docs and skills | Model-facing usage guidance and contributor architecture guidance | Private alternative contracts that differ from generated schemas |

After the catalog-authoritative machine branch lands, semantic machine changes
start in the catalog DSL and are generated outward. Production-only machine state
is forbidden. This ADR requires a catalog batch for projection snapshot binding
and projection-triggered terminals; that batch must not be deferred to runtime
glue.

### Deliverable 4: Buildable implementation slices and gates

The implementation sequence is a set of buildable vertical slices. A slice may
touch core, providers, builtin code, wire, SDKs, and docs together when that is
what keeps the workspace compiling. The phase names below describe the primary
semantic objective, not isolated crate ownership. Do not remove a public/core
type in one phase while deferring its downstream compile repairs to a later
phase.

| Slice | Primary objective | Required same-slice mechanical work | Gate |
| --- | --- | --- |
| 1 | Canonical transcript model and public contract cutover | Update every Rust call site, provider serializer branch, wire schema, generated SDK type, SDK parser, CLI/history renderer, and REST/RPC/MCP history surface needed for `make check` to compile after removing `Message::BlockAssistant` and old assistant block variants. Generated-image emission is explicitly disabled until Slice 6. | `make check` plus focused serde/session/provider-builder/surface/SDK tests that accept only the new canonical assistant, reject old flat/block assistant rows, and prove `generate_image` fails through the existing generic tool-failure path while the image-operation machine protocol is absent. |
| 2 | Structural content traversal | Update hydration, externalization, blob reachability, compaction, indexing, hooks, and SDK convenience views in the same slice. Provider replay selection remains disabled. | focused tests proving user, assistant, tool-result, deferred prompt, and deferred tool-result images hydrate, externalize, and collect blob ids through structural walkers without asserting provider replay selection. |
| 3 | Catalog machine projection semantics | Add catalog DSL changes and generated production artifacts in one batch; remove any production-only runtime state introduced as temporary scaffolding. | machine codegen, drift check, runtime schema parity, runtime alphabet parity, and TLC verification for projection binding and projection-triggered terminals. |
| 4 | Image projection snapshot and provider-consumption preflight | Wire runtime materialization, provider profile preflight, typed denials, durable receipts, and recovery paths together. | projection/preflight tests for explicit sources, typed latest-visible selectors, omitted items, missing blobs, oversized media, compaction boundaries, provider-consumption denials, and deterministic rebuild. |
| 5 | Provider lowering and replay | Update OpenAI, Gemini, Anthropic, OpenAI-compatible, realtime/text request builders and provider replay codecs. | provider request-builder tests proving approved canonical content serializes without silent drops; semantic denials are Slice 4 preflight outcomes, and builders report only invariant or mechanical serialization failures. |
| 6 | Builtin image generation commit path | Wire tool-batch transcript commit, generated image handles, audit rows, replay sidecars, external artifact handles, and adjacency behavior together. | adjacency tests for `AssistantToolBatch`, parallel tool calls, and negative tests proving realtime/direct external dispatch uses `ExternalArtifactOnly` without assistant transcript append. |
| 7 | Docs, examples, and skills | Update README, docs, guide examples, and `.claude/skills/meerkat-*`; public schemas, surfaces, and SDK types were already cut over in Slice 1. | docs/skills reference checks and snippet checks using the canonical transcript/tool shape. |
| 8 | End-to-end live smokes | Add live OpenAI/Gemini, SDK, and cross-provider replay scenarios after slices 1-7 are green. | OpenAI/Gemini live lanes, SDK live lanes, and nasty scenario 76 blob-hash replay proof. |

Normal broad lanes should use the repository Make surface:

```bash
make check
make lint
make test
make e2e-fast
make e2e-system
make e2e-smoke
```

BuildBuddy may be used through the same Make targets with
`MEERKAT_BUILDBUDDY=1` when available, but Cargo-backed Make lanes remain the
fallback.

### Deliverable 5: Regression test matrix

| Test | Prevents |
| --- | --- |
| Old flat assistant fixture is rejected with a typed cutover error | Accidentally preserving the old assistant ontology. |
| Old `block_assistant` image fixture is rejected with a typed cutover error | Accidentally preserving the old generated-image ontology or inventing provenance. |
| New canonical assistant serializes without `role: "block_assistant"` | Reintroducing the split transcript ontology. |
| Assistant `ContentBlock::Image` externalizes, hydrates, and contributes blob reachability | Repeating the original role-specific traversal bug. |
| Missing selected assistant image blob terminalizes through the machine | Silent fallback when bytes are unavailable. |
| Projection/provider-consumption denial persists a `ProjectionAttemptRecord` and recovers the same terminal | Ephemeral projection/preflight failures changing after crash. |
| Provider params/planning denials persist receipts and recover the same terminal | Provider crates or runtime helpers owning early denial semantics. |
| Snapshot-bound projection persists preflight input, preflight receipt, and `ProviderConsumptionPlanReceipt` | Transient approved provider plans changing during recovery. |
| Mixing concrete image handles and latest-visible selectors in one image set fails validation typed | Accidentally editing the wrong visible image. |
| Typed `LatestVisibleAssistantImage` selector resolves most recent eligible assistant image and audits it | Provider-local guessing and unprovable continuity. |
| Unverifiable source/projection observation denies before selection | Shell-supplied incomplete candidate sets influencing selector/projection semantics. |
| Compacted prior image without retained replay anchor fails typed for implicit selection | Pretending hidden compacted content is still visible context. |
| Retained image anchors require machine-created `RetainedImageAnchorRecord` | Store or compactor inventing selectable hidden media. |
| OpenAI Responses request builder receives selected assistant image bytes | Dropping assistant images on OpenAI replay. |
| Provider preflight sees inspected media facts before approving selected image | Request builder discovering provider-consumption denial first. |
| OpenAI Images API executor receives explicit source/reference image bytes or rejects typed | Treating image API calls like full transcript replay. |
| Gemini request builder receives selected assistant image bytes/continuity reference | Regenerating from text instead of editing the prior image. |
| Anthropic request builder role-adapts or rejects selected assistant image | Illegal assistant-role image replay. |
| Provider serializers skip no selected content silently and emit no empty assistant/model message | Invalid provider transcripts from image-only assistant messages. |
| Tool-use -> tool-result -> assistant image ordering holds for normal dispatch | Breaking provider tool-call adjacency. |
| Mixed image/non-image tool batch recovers from participant receipts without re-executing completed tools | Losing non-image tool results while preserving image-operation receipts. |
| Failed image operation contributes `ImageOperationTerminalCommitPart` with failure tool result | Blocking sibling tool results when image generation terminalizes before output. |
| Provider dispatch serialization failure terminalizes before `ProviderCallAttemptReceipt` | Treating local serialization defects as unknown external provider outcomes. |
| Crash after `ProviderCallAttemptReceipt` but before `ProviderOutputReceipt` terminalizes unknown outcome or resumes from durable output | Re-calling the provider blindly or losing generated bytes during recovery. |
| Provider proves no output for a staged attempt and machine either retries under budget or terminalizes no-output retry exhausted | Provider/shell choosing retry semantics. |
| Provider output receipt with text/refusal but no image terminalizes `ProviderOutputWithoutImage` | Shell/provider code classifying text-only image responses. |
| Replay attachments are written atomically with transcript/audit rows using allocated block refs | Replay status surviving without the provider replay payload. |
| Replay sidecar expiry/loss records machine-owned replay-material-lost decision | Storage retention state silently changing replay semantics. |
| Transcript append retry consumes the same `TranscriptAppendAllocationReceipt` | Message/block ids or ordering changing across recovery. |
| Parallel tool calls append all generated image content after the matching `ToolResults` batch | Interleaving generated images between tool calls and results. |
| Staged/realtime/direct tool dispatch preserves adjacency or refuses image transcript effects | A non-normal dispatch path bypassing the adjacency fix. |
| SDKs expose generated image convenience views from assistant content image blocks | SDKs depending on removed `AssistantBlock::Image`. |
| Scenario 76 trace proves first blob hash -> Gemini edit request -> edited blob hash -> OpenAI readback request | Passing live smoke by inference from prompt text instead of real image replay. |

### Compaction, memory, and indexing policy

Generated image replay must remain coherent in long sessions:

- Compaction may summarize image context as text for normal language continuity,
  but text summaries are not image bytes and must not satisfy selected image
  replay.
- Explicit assistant image handles resolve through
  `GeneratedImageHandleRecord::Assistant`, not directly through audit rows or
  blob ids. Compaction may hide the visible transcript block, but the handle
  remains valid only while that machine-created record points to reachable media
  and retention policy allows it.
- Typed latest-visible selectors resolve from visible eligible assistant image
  content only. Hidden compaction-retained anchors are not visible content and
  are not candidates for `LatestVisibleAssistantImage`.
- Compaction may create explicit retained image anchors. Those anchors require a
  separate typed handle, such as `RetainedImageAnchor`, and resolve through the
  same shared domain resolver as assistant image ids. Anchor creation is a
  catalog-machine retention decision emitted during compaction planning; the
  store persists anchor rows and retention epochs as evidence, but does not
  decide which hidden media becomes selectable.
- Semantic indexing may index captions, provider text, revised prompts, or user
  descriptions, but indexes are retrieval aids, not media truth.
- Provider text and revised prompts belong in audit/provenance. They are not
  visible assistant content unless the assistant actually emits them as content.
- Blob retention and garbage collection mechanically enforce structural
  reachability and machine retention decisions; they must not create semantic
  visibility, replay eligibility, or source eligibility from provider metadata or
  store policy alone.

### Blast radius

The rewrite is a wire and surface change, not a local type cleanup.

High-risk Rust areas:

- `meerkat-core/src/types.rs`: message variants, assistant block variants, user
  and assistant metadata, cutover validation.
- `meerkat-core/src/session.rs`: assistant append helpers, text extraction,
  tool-call iteration, and external assistant append.
- `meerkat-core/src/agent/state.rs` and `meerkat-core/src/agent/runner.rs`:
  assistant message construction and session-effect ordering.
- `meerkat-core/src/image_content.rs`: structural traversal over all
  `ContentBlock`s.
- `meerkat-session/src/compactor.rs`: media stripping and image replay policy
  through compaction.
- `meerkat-openai`, `meerkat-gemini`, and `meerkat-anthropic`: provider
  request builders and opaque provider replay-envelope codecs.
- `meerkat-tools/src/builtin/image_generation.rs`: generated assistant content
  append and provenance metadata. During Slices 1-5 the builtin must terminalize
  typed as unavailable instead of using the removed `BlockAssistant` append path
  or synthesizing the new generated-image handle path before the machine owns it.
- `meerkat-contracts/src/wire/session.rs`: public history schema.
- `meerkat-store`: persisted session JSON cutover errors or explicit offline
  migration tooling.

High-risk surface areas:

- REST/RPC/MCP session history endpoints;
- CLI history rendering and generated image display;
- Python and TypeScript generated types and convenience wrappers;
- SDK e2e smoke helpers that currently accept both `assistant` and
  `block_assistant`, which must be updated to require canonical `assistant`;
- docs, README, and `.claude/skills/meerkat-*` references.

### Ownership boundaries

After the catalog/production machine parity branch lands:

- new lifecycle phases, terminal classes, approval facts, replay-selection
  facts, or recovery semantics must start in the catalog machine DSL;
- provider model ownership, provider-specific parameters, default image models,
  backend selection, and provider validation belong in provider profiles;
- runtime/factory code may compose provider profiles, but must not hardcode
  provider model-name folklore as semantic truth;
- provider executors lower selected requests and return evidence; they do not
  commit blobs, mutate transcripts, or transition the machine;
- projection selection policy is catalog-machine-owned, not provider adapter or
  runtime policy;
- hydration and externalization are structural content operations, not
  role-specific special cases.

### Implementation gates

Each slice gate should be green before moving to the next slice. If a semantic
change cannot compile without downstream call-site or surface edits, those edits
belong in the same slice:

1. **Contract gate**: committed target Rust/wire shape with serde fixtures that
   accept canonical assistant rows and reject old flat/block assistant rows.
   Until the image-operation machine protocol lands, `generate_image` must fail
   through the existing generic tool-failure path rather than inventing
   image-operation terminal semantics or emitting shell-owned generated image
   transcript effects.
2. **Traversal gate**: assistant, user, and tool-result images externalize,
   hydrate, and collect blob ids through the same helper.
3. **Machine gate**: projection snapshot binding and projection-triggered
   terminal classes are catalog-owned, generated, and verified before runtime
   projection behavior can rely on them.
4. **Projection gate**: a selected assistant-generated blob image reaches
   OpenAI, Gemini, and Anthropic provider request builders as image bytes or a
   machine-owned provider-consumption denial, never as silent placeholder text.
5. **Adjacency gate**: generated assistant content remains after tool results,
   with regression tests for tool-use/tool-result adjacency.
6. **Provider replay gate**: Gemini thought signatures, Anthropic thinking
   signatures, OpenAI reasoning behavior, and generated-image provenance keep
   their current provider-specific invariants through replay attachments and
   audit-only trace evidence.
7. **Documentation gate**: docs, README, and skills describe the canonical
   transcript/tool shape already exposed by the Slice 1 public contract cutover.
8. **Live gate**: the nasty cross-provider smoke proves replay with artifact
   evidence: first generated blob hash -> Gemini edit request -> edited blob
   hash -> OpenAI readback request.

## Current Mechanism

### Model-facing tool

Meerkat exposes one privileged builtin tool:

```text
generate_image(request)
```

The active session model calls `generate_image` when it wants an image. The tool
has universal typed fields such as prompt, provider/model target, size, quality,
format, count, source/reference images, typed visible-history selectors, and a
provider-owned parameter envelope.

Provider-specific image parameters are intentionally not normalized into a
single cross-provider schema. The universal fields cover common intent. Provider
crates own the exact image models, supported model-specific parameters, default
models, backend selection, validation, and documentation for their targets. The
wire shape is still typed:

```rust
pub struct ProviderImageParamsEnvelope {
    pub provider: ProviderId,
    pub schema: ProviderImageParamsSchema,
    pub schema_version: ProviderImageParamsSchemaVersion,
    pub payload: Box<RawValue>,
}
```

Provider crates own the envelope schema, schema version, and validation. Invalid
provider parameters are recorded as `ProviderParamsDenialReceipt` with
provider/schema version and evidence digest, then the machine terminalizes with
the typed provider-params denial. Provider planning denial is likewise recorded as
`ProviderPlanningDenialReceipt` with provider profile version, target model, and
evidence digest before machine terminalization. Raw JSON is never interpreted by
runtime, surfaces, or the machine.

The tool is privileged rather than an ordinary external MCP tool because image
generation participates in session lifecycle, scoped model routing, provider
context projection, blob commits, audit, and transcript mutation.

### Provider planning

Image target planning is provider-owned and runtime-composed.

At dispatch time the builtin asks the provider-owned image-generation planner to
resolve the request into a typed plan. Current code may expose provider-specific
execution-family variants to shared crates; that is current mechanism, not the
target ownership boundary.

The important architectural rule is that provider-specific knowledge lives in
provider crates:

- OpenAI profiles know which OpenAI image models exist, which backend should be
  used, and which provider parameters are valid.
- Gemini profiles know which Gemini models are image-capable, whether scoped
  model execution is required, and which provider parameters are valid.
- The runtime composes these profiles. It should not hardcode provider naming
  folklore such as `gpt-image*`, `dall-e*`, or `gemini-*` as semantic truth.

The target shared boundary must not match those provider-specific plan variants
as semantic truth. Shared code may carry an opaque provider plan envelope plus a
small generic summary:

```rust
pub struct ResolvedImageExecutionPlan {
    pub provider: ProviderId,
    pub target_model: ModelId,
    pub scoped_execution: ImageScopedExecutionRequirement,
    pub provider_plan_schema: ProviderPlanEnvelopeSchema,
    pub provider_plan_schema_version: ProviderPlanEnvelopeSchemaVersion,
    pub provider_plan: ProviderImageExecutionPlanEnvelope,
}
```

Provider crates own the envelope schema, schema version, and backend selection.
Audit and projection snapshots may store the generic summary and opaque envelope
id/digest, but core/runtime must not branch on OpenAI/Gemini backend variant
names except through typed provider-profile output such as
`ImageScopedExecutionRequirement`.
Accepted planning is a durable handoff, not a transient function return. The
provider planner returns a `ResolvedImageExecutionPlan`; the shell persists it as
`ProviderPlanningReceipt`, and the machine observes the resulting
`ProviderPlanningReceiptRef` before source resolution, projection, scoped
override binding, or provider execution can proceed. Recovery reloads the receipt
by id and digest. It never replans merely because runtime memory was lost.

```rust
put_provider_planning_receipt(session_id, receipt)
get_provider_planning_receipt(session_id, receipt_id)
put_provider_planning_denial_receipt(session_id, receipt)
get_provider_planning_denial_receipt(session_id, receipt_id)
```

```rust
pub enum ImageScopedExecutionRequirement {
    UsesSessionModel,
    RequiresImageModelTarget {
        target_provider: ProviderId,
        target_model: ModelId,
    },
}

pub struct ImageScopedOverrideBinding {
    pub target: EffectiveModelTarget,
    pub restore: RestoreEffectiveModelTarget,
    pub topology_epoch: TopologyEpoch,
    pub fence: ScopedOverrideFence,
}
```

The provider profile may say that a scoped image-model target is required. It
does not choose restore targets, topology epochs, fences, or current effective
model bindings. The machine/runtime seam converts
`ImageScopedExecutionRequirement` into an `ImageScopedOverrideBinding` using the
current machine-owned model-routing state, and the machine owns apply/restore
lifecycle. Runtime code must not reduce scoped override semantics to a boolean.

### Operation lifecycle

`generate_image` dispatch creates an image operation. The Meerkat machine owns
operation state and terminalization through catalog DSL fields, inputs, effects,
and handlers rather than ad hoc runtime enums. The implementation adds generated
maps keyed by `ImageOperationId` for operation status, provider-params denials,
approved/denied provider planning receipts, source-resolution decisions,
projection attempts, scoped
override bindings, provider dispatch/call/output receipts, blob commit receipts,
tool-batch commit state, and terminal outcomes. It adds catalog input variants
and generated handlers for begin, provider-params denied, planning denied, source
resolved/denied, projection attempt recorded, provider consumption denied,
scoped override applied/restored/failed, provider dispatch serialized/failed,
provider attempt staged, provider output/no-output/unknown-outcome observed,
blob commit recorded/failed, batch commit recorded/failed, and terminalization.
Closed-world effects are emitted for shell mechanics such as provider execution,
blob writes, and store receipt writes.

Until the generated catalog phases and tool-batch commit protocol exist,
`generate_image` is intentionally unavailable. The builtin may parse requests so
call sites compile, but before Slice 3 it must fail through the existing generic
tool-failure path, because no image-operation machine terminal class exists yet.
After Slice 3 introduces image-operation terminal classes and before Slice 6
introduces the commit protocol, it may use those generated terminal classes. It
must not keep the old `BlockAssistant` append path alive and must not synthesize
`GeneratedImageHandleRecord` rows from shell code. That makes Slices 1-5
buildable but not image-generation feature-complete. Slice 6 is the first slice
where generated images may become transcript-visible assistant content.

The ownership rule is:

- the machine owns phase truth;
- the shell executes provider calls and blob writes;
- provider outputs are evidence, not semantic authority;
- all terminal classes are typed.

If any of the semantic facts above exist in core/runtime but not in the catalog
machine, that is an implementation debt, not an accepted architecture. The
target implementation must close that gap catalog-first before relying on those
facts for runtime behavior.

OpenAI plans do not activate a scoped model override. Gemini native image-model
plans may activate an operation-scoped model override while the provider call is
running, then restore the previous effective model.

### Current execution flow

The current dispatch flow has been moving toward:

1. begin the image operation in the machine;
2. normalize and validate the model-facing request;
3. resolve a provider-owned execution plan;
4. terminalize through the machine if validation or planning is denied;
5. activate an operation-scoped override if the plan requires it;
6. call the provider image executor;
7. commit returned image bytes to the blob store;
8. restore any scoped override;
9. durably commit audit/tool-result/assistant-image transcript effects;
10. complete the image operation with a typed terminal class.

The "after the tool result" rule is load-bearing. Provider transcripts require
tool-call/tool-result adjacency. Generated assistant image content must not be
inserted between an assistant tool call and its tool result.

### Current transcript shape

The current branch has two assistant message shapes:

```rust
enum Message {
    System(SystemMessage),
    SystemNotice(SystemNoticeMessage),
    User(UserMessage),
    Assistant(AssistantMessage),        // legacy flat assistant
    BlockAssistant(BlockAssistantMessage),
    ToolResults { results: Vec<ToolResult> },
}
```

`UserMessage` already supports multimodal content through `ContentBlock`:

```rust
pub struct UserMessage {
    pub content: Vec<ContentBlock>,
    pub render_metadata: Option<RenderMetadata>,
}

pub enum ContentBlock {
    Text { text: String },
    Image { media_type: String, data: ImageData },
    Video { media_type: String, duration_ms: u64, data: VideoData },
}
```

Ordered assistant output uses `BlockAssistantMessage`:

```rust
pub struct BlockAssistantMessage {
    pub blocks: Vec<AssistantBlock>,
    pub stop_reason: StopReason,
}

pub enum AssistantBlock {
    Text { text: String, meta: Option<Box<ProviderMeta>> },
    Reasoning { text: String, meta: Option<Box<ProviderMeta>> },
    ToolUse { id: String, name: String, args: Box<RawValue>, meta: Option<Box<ProviderMeta>> },
    Image { image_id: AssistantImageId, blob_ref: BlobRef, ... },
}
```

That split is the current architectural smell:

- user images are `ContentBlock::Image`;
- tool-result images are `ContentBlock::Image`;
- generated assistant images are `AssistantBlock::Image`;
- hydration and provider projection already know how to handle
  `ContentBlock::Image`;
- assistant image blocks are stored and displayed, but they are not treated as
  the same canonical media content during replay.

### Current replay gap

The current image replay behavior is incomplete.

Generated images are durable. They are committed to the blob store and appear in
session history as assistant image blocks. However, when a later provider call is
built, the normal hydration path hydrates images inside user messages and tool
results. It does not hydrate generated assistant images because those images are
not `ContentBlock::Image`.

Provider adapters also lower user image content to provider-native shapes, but
they skip or text-project assistant image blocks.

Consequences:

- a follow-up prompt such as "modify the existing image" may not receive the
  prior image bytes;
- Gemini can regenerate from text instead of editing the prior bitmap;
- OpenAI can receive text references rather than image inputs unless the image is
  explicitly passed through the source/reference image path;
- untyped image-continuity prompts can appear to work while actually depending
  on model inference rather than transcript image replay.

Explicit `source_images` and `reference_images` are still meaningful and should
remain. The problem is that visible assistant image content should also be a
normal content block when it is selected for replay.

## Target Architecture

### Canonical transcript model

The target transcript model has one assistant message shape and one media
content representation:

```rust
pub enum Message {
    System(SystemMessage),
    SystemNotice(SystemNoticeMessage),
    User(UserMessage),
    Assistant(AssistantMessage),
    ToolResults(ToolResultsMessage),
}

pub struct UserMessage {
    pub blocks: Vec<ContentBlockEntry>,
    pub meta: UserMessageMeta,
}

pub struct AssistantMessage {
    pub blocks: Vec<AssistantBlock>,
    pub meta: AssistantMessageMeta,
}
```

There is no `Message::BlockAssistant`. There is no legacy flat
`AssistantMessage`. Ordered assistant blocks are the assistant message format.

`ContentBlock` is the only representation of visible multimodal content.
`BlockId` is a transcript-owned stable domain handle assigned at append time for
each persisted content/control block:

```rust
pub struct ContentBlockEntry {
    pub block_id: BlockId,
    pub block: ContentBlock,
}
```

```rust
pub enum ContentBlock {
    Text {
        text: String,
    },
    Image {
        media_type: String,
        data: ImageData,
    },
    Video {
        media_type: String,
        duration_ms: u64,
        data: VideoData,
    },
}

pub enum ImageData {
    Blob { blob_id: BlobId },
}
```

Canonical persisted transcript media is blob-backed. Surfaces and SDK ingress may
accept inline image bytes as request material, but the session append boundary
externalizes them first and records an `ImageExternalizationReceipt`; an inline
image that cannot be externalized never becomes a selectable transcript block.
Projection therefore never performs hidden inline-to-blob mutation.

```rust
pub struct ImageExternalizationReceipt {
    pub receipt_id: ImageExternalizationReceiptId,
    pub ingress_block_ref: IngressContentBlockRef,
    pub blob_id: BlobId,
    pub media_type: MediaType,
    pub byte_sha256: String,
    pub byte_len: u64,
}
```

Assistant output is ordered by `AssistantBlock`, but assistant-visible content
uses `ContentBlock`:

```rust
pub enum AssistantBlock {
    Content {
        block_id: BlockId,
        block: ContentBlock,
        origin: AssistantContentOrigin,
    },
    Reasoning {
        block_id: BlockId,
        text: String,
        replay: ProviderReplayRef,
    },
    ToolUse {
        block_id: BlockId,
        id: String,
        name: String,
        args: Box<RawValue>,
        replay: ProviderReplayRef,
    },
}
```

The invariant is:

> If it is visible text, image, video, or future media, it is a `ContentBlock`.
> If it is assistant-only control or replay structure, it is an
> `AssistantBlock` variant.

Assistant visible-content origin is explicit and non-optional:

```rust
pub enum AssistantContentOrigin {
    ModelOutput {
        replay: ProviderReplayRef,
    },
    GeneratedImage {
        operation_id: ImageOperationId,
        image_id: AssistantImageId,
        output_index: u32,
        replay: ProviderReplayRef,
    },
}

pub struct ProviderReplayEnvelope {
    pub handle: ProviderReplayHandle,
    pub provider: ProviderId,
    pub schema: ProviderReplayEnvelopeSchema,
    pub schema_version: ProviderReplayEnvelopeSchemaVersion,
    pub payload: Box<RawValue>,
}

pub enum ProviderReplayOwnerRef {
    TranscriptBlock(TranscriptBlockRef),
    RetainedImageAnchor(RetainedImageAnchorId),
}

pub struct ProviderReplayAttachment {
    pub owner: ProviderReplayOwnerRef,
    pub handle: ProviderReplayHandle,
    pub provider: ProviderId,
    pub schema: ProviderReplayEnvelopeSchema,
    pub schema_version: ProviderReplayEnvelopeSchemaVersion,
    pub payload: Box<RawValue>,
}

pub struct ProviderReplayAttachmentDigest {
    pub owner: ProviderReplayOwnerRef,
    pub handle: ProviderReplayHandle,
    pub provider: ProviderId,
    pub schema: ProviderReplayEnvelopeSchema,
    pub schema_version: ProviderReplayEnvelopeSchemaVersion,
    pub digest: ReplayAttachmentDigest,
}

pub struct ProviderReplayBindingRef {
    pub owner: ProviderReplayOwnerRef,
    pub handle: ProviderReplayHandle,
    pub provider: ProviderId,
    pub schema: ProviderReplayEnvelopeSchema,
    pub schema_version: ProviderReplayEnvelopeSchemaVersion,
    pub digest: ReplayAttachmentDigest,
}

pub struct ProviderTraceEvidence {
    pub provider: ProviderId,
    pub schema: ProviderTraceSchema,
    pub schema_version: ProviderTraceSchemaVersion,
    pub digest: ProviderTraceDigest,
}

pub enum ProviderReplayRef {
    NotApplicable,
    SidecarExpected(ProviderReplayBindingRef),
    IntentionallyOmitted {
        decision: ProviderReplayOmissionDecisionRef,
        reason: ProviderReplayOmissionReason,
    },
}

pub struct ProviderReplayOmissionDecisionRef {
    pub decision_id: MachineDecisionId,
    pub decision_event_id: MachineEventId,
    pub decision_digest: ProviderReplayOmissionDecisionDigest,
}

pub enum ProviderReplayStatus {
    NotApplicable,
    Available,
    IntentionallyOmitted {
        reason: ProviderReplayOmissionReason,
    },
    Missing {
        reason: ProviderReplayMissingReason,
    },
}
```

`AssistantContentOrigin` is not optional. A newly generated assistant image
without `operation_id`, `image_id`, and `output_index` is invalid. The origin
does not own blob identity, media type, provider response IDs, provider text,
warnings, or revised prompts. Blob identity and media type belong to
`ContentBlock::Image`. Provider/operation evidence belongs to the
image-generation audit record described below.

`ProviderReplayEnvelope` / `ProviderReplayAttachment` are internal session
replay storage, not public transcript wire. Canonical transcript blocks and
retained anchors store only `ProviderReplayRef`, which is append-time replay
provenance: not applicable, sidecar expected with concrete
handle/provider/schema/schema-version/digest binding, or intentionally omitted
with a `ProviderReplayOmissionDecisionRef`. The provider-owned payload lives in
sidecar replay-attachment storage keyed by `ProviderReplayOwnerRef`;
transcript/anchor truth stores the `ProviderReplayBindingRef`, not the payload.
Public REST/RPC/MCP/SDK history exposes a redacted `ProviderReplayStatus` read
projection with only domain status and reason. It never exposes machine event ids,
machine decision refs, replay owner refs, or sidecar handles. The projection
is computed by joining `ProviderReplayRef` with the machine-owned sidecar
retention state. Replay sidecar expiry/loss is not a raw storage fact that
changes semantics by itself. The machine records a replay-material-lost decision
with retention policy version and affected `ProviderReplayOwnerRef`s. The
redacted status becomes `Missing` only when that machine decision exists.
Verified storage corruption is evidence submitted to a typed machine transition;
it is not itself a surfaced replay status and cannot be used directly by provider
projection.
Provider projection terminalizes through the typed replay-material-lost path when
selected content requires lost replay material.
Provider crates own the payload schema and codecs; adding a provider-specific
replay token must not add a provider-specific enum variant to core transcript or
public wire types. Whether a missing replay handle is required for a given
provider/model/request is not transcript truth; it is decided by machine-bound
projection and provider-profile preflight for that target.

### Message metadata

Loose message fields become typed metadata:

```rust
pub struct UserMessageMeta {
    pub render: Option<RenderMetadata>,
    pub origin: Option<MessageOrigin>,
}

pub struct AssistantMessageMeta {
    pub stop_reason: StopReason,
    pub audit: Option<AssistantMessageAuditMeta>,
}
```

`UserMessageMeta::render` replaces the current dangling
`render_metadata: Option<RenderMetadata>`.

`AssistantMessageMeta::stop_reason` replaces the current dangling
`stop_reason: StopReason`.

Usage remains session-accounting-owned by default. If a future surface needs
per-message usage, it must be optional audit metadata under
`AssistantMessageMeta::audit`, not a second billing source of truth.

### Cutover ownership

This branch intentionally has no runtime read compatibility for removed
transcript shapes. `meerkat-core` defines the canonical transcript schema and the
single `TranscriptCutoverError` taxonomy for old flat assistant rows, old
`block_assistant` rows, and old `UserMessage.content` rows. Store/session load
applies that validator to persisted session rows. `meerkat-contracts` applies the
same validator at public wire decode boundaries. Serde helpers must not silently
normalize old rows into the new shape, and runtime, factory, surfaces, and SDKs
must not carry independent migration/read-compatibility branches. Any archival
conversion is an explicit offline tool that rewrites storage before runtime load.

### Generated image content

Generated image bytes are stored in the blob store and represented in transcript
history as normal assistant content:

```rust
Message::Assistant(AssistantMessage {
    blocks: vec![
        AssistantBlock::Content {
            block_id,
            block: ContentBlock::Image {
                media_type: "image/png".to_string(),
                data: ImageData::Blob { blob_id },
            },
            origin: AssistantContentOrigin::GeneratedImage {
                operation_id,
                image_id,
                output_index,
                replay,
            },
        },
    ],
    meta: AssistantMessageMeta {
        stop_reason: StopReason::EndTurn,
        ...
    },
})
```

Generated-image evidence is stored separately:

```rust
pub struct ImageGenerationAuditRecord {
    pub operation_id: ImageOperationId,
    pub session_id: SessionId,
    pub request_digest: RequestDigest,
    pub provider_params_digest: ProviderParamsDigest,
    pub provider_plan_ref: ProviderPlanningReceiptRef,
    pub provider_plan_digest: ProviderPlanDigest,
    pub images: Vec<ImageGenerationAuditImage>,
    pub provider_text: ProviderTextDisposition,
    pub revised_prompt: RevisedPromptDisposition,
    pub provider_trace: Vec<ProviderTraceEvidence>,
    pub replay_attachment_digests: Vec<ProviderReplayAttachmentDigest>,
    pub warnings: Vec<ImageGenerationWarning>,
}

pub struct ImageGenerationAuditImage {
    pub image_id: AssistantImageId,
    pub output_index: u32,
    pub provider_reported_width: Option<u32>,
    pub provider_reported_height: Option<u32>,
    pub committed_blob_id: BlobId,
    pub byte_sha256: String,
}
```

Audit records live in session-scoped audit storage, not inside transcript
messages and not inside provider blobs. The session store owns persistence,
migration, retention, and query APIs for `ImageGenerationAuditRecord`:

```rust
put_image_generation_audit(session_id, record)
get_image_generation_audit(session_id, operation_id)
list_image_generation_audits(session_id, range)
```

Transcript content may point to audit records through
`AssistantContentOrigin::GeneratedImage`, but transcript rendering must remain
possible without eagerly loading every audit record. Audit rows are retained at
least as long as any reachable transcript content or explicit image handle points
to their `operation_id` / `image_id`. Longer mechanical retention may be store
configuration, but it does not extend semantic image-source eligibility, replay
eligibility, or visibility without a machine retention decision.

Audit records are immutable provider/output evidence. They store request and
planning refs/digests for correlation, not full request or plan authority. They
never store terminal classes, terminal event ids, approvals, scoped-override
phases, or recovery truth. Recovery reads machine state and machine event history
first. Surfaces that need terminal state join audit evidence with machine
projections at read time; a disagreement is an audit/projection corruption
signal, not a second source of lifecycle truth.

The architectural rule is:

- image bytes and media type live in `ContentBlock::Image`;
- generated-image provenance does not create a second image content type;
- operation/provider evidence lives in session-scoped
  `ImageGenerationAuditRecord`;
- dimensions are provider/audit evidence unless independently verified from the
  committed bytes;
- `AssistantImageId` is only for assistant transcript-visible generated images;
- `GeneratedImageHandleRecord` is the only canonical resolver from
  `(SessionId, GeneratedImageHandle)` to image handle location. `AssistantImageId`
  is one handle variant for assistant transcript images; external artifact
  outputs use `ExternalImageArtifactHandle`;
- media facts are loaded from the resolved location's canonical content or anchor
  record; audit rows and blob ids are evidence, not resolver authority;
- `BlobRef` remains a surface projection assembled from blob identity and media
  type, not an independent source of transcript truth.

```rust
pub enum RetainedImageAnchorSource {
    Transcript {
        transcript_ref: TranscriptBlockRef,
    },
    ExternalImageArtifact {
        commit_id: ExternalImageArtifactCommitId,
        operation_id: ImageOperationId,
        output_index: u32,
    },
}

pub enum GeneratedImageHandle {
    Assistant(AssistantImageId),
    ExternalArtifact(ExternalImageArtifactHandle),
}

pub struct GeneratedImageHandleRecord {
    pub session_id: SessionId,
    pub handle: GeneratedImageHandle,
    pub operation_id: ImageOperationId,
    pub location: GeneratedImageHandleLocation,
    pub handle_epoch: GeneratedImageHandleEpoch,
    pub retention_epoch: RetentionEpoch,
    pub created_by_decision: MachineDecisionId,
}

pub enum GeneratedImageHandleLocation {
    Transcript { transcript_ref: TranscriptBlockRef },
    RetainedAnchor { anchor_id: RetainedImageAnchorId },
}

pub enum GeneratedImageHandleResolutionFailure {
    MissingHandleRecord,
    HandleKindLocationMismatch,
    TranscriptRefMismatch,
    RetainedAnchorMismatch,
    MediaLocationUnavailable,
    RetentionExpired,
}
```

The handle record is control identity, not a second media store, and it has
exactly one canonical location. Assistant outputs initially resolve through
`GeneratedImageHandleLocation::Transcript`; external artifact-only outputs
resolve through `GeneratedImageHandleLocation::RetainedAnchor`. If compaction
retains an assistant image while hiding the transcript block, the machine
supersedes the handle record with a new `handle_epoch` pointing to the retained
anchor. There is never a dual `TranscriptAndRetainedAnchor` resolver. Resolution
first proves session ownership and location validity, then reads media identity
from `ContentBlock::Image` or `RetainedImageAnchorRecord`. A mismatch between the
handle location and the canonical media location is a typed resolution failure.

### Hydration and externalization

Hydration and externalization must walk all `ContentBlock`s wherever they appear:

- user message blocks;
- assistant content blocks;
- tool-result content blocks;
- deferred prompt input;
- deferred tool results;
- future transcript projections that carry content.

The hydration API should be content-structural rather than role-special:

```rust
hydrate_message_content(message)
externalize_message_content(message)
```

Those helpers recurse into message-specific containers and operate only on
`ContentBlock` media data. They do not need to know whether an image came from a
user upload, tool result, or assistant image generation.

### Provider projection

Provider adapters translate selected transcript content into provider wire
shapes. They do not decide what content exists or which history is selected.

Projection policy ownership:

- the catalog machine owns the complete `ImageProjectionDecision`: policy
  version, transcript boundary, source-resolution decisions, selected canonical
  manifest, omitted canonical manifest, image-source subviews,
  truncation/compaction decision ids, replay binding refs/digests, ordering, and
  staleness/rebuild trigger;
- the machine decision names transcript/source references and purposes, not
  hydrated bytes;
- the shell materializes the projection snapshot from that exact decision and
  store/blob observations;
- if materialization cannot reproduce the machine-owned manifest, the machine
  terminalizes through projection mismatch/failure.

The machine can own the complete projection only if it receives a complete
mechanical observation of the candidate history. Image-source resolution is one
sub-protocol for edit/source authority; it is not the whole projection input.
Before emitting `ImageProjectionDecision`, the shell supplies
`ProjectionHistoryObservation`: a verified transcript-boundary inventory of
candidate content/control blocks, tool-adjacency groups, typed token-accounting
facts, compaction/visibility/retention facts, blob-backed media budget facts, and
replay availability. The machine chooses selected and omitted canonical manifest
items from that observation. Projection materialization may verify and hydrate;
it may not discover additional candidates or silently change membership. Token
estimates are not free-form shell hints: the observation names the estimator id,
estimator version, estimator digest, provider profile version, token policy
version, source digests, and rebuild trigger that make the estimates valid.

```rust
pub struct ProjectionHistoryObservationRef {
    pub observation_id: ProjectionHistoryObservationId,
    pub observation_digest: ProjectionHistoryObservationDigest,
}

pub struct ProjectionHistoryObservation {
    pub observation_id: ProjectionHistoryObservationId,
    pub operation_id: ImageOperationId,
    pub transcript_boundary: TranscriptBoundary,
    pub candidates: Vec<ProjectionCandidateObservation>,
    pub tool_adjacency_groups: Vec<ToolAdjacencyGroupObservation>,
    pub token_accounting: ProjectionTokenAccountingSnapshot,
    pub compaction_epoch: CompactionEpoch,
    pub visibility_epoch: VisibilityEpoch,
    pub retention_epoch: RetentionEpoch,
    pub proof: ProjectionHistoryObservationValidity,
    pub observation_digest: ProjectionHistoryObservationDigest,
}

pub enum ProjectionCandidateObservation {
    TextContent {
        block_ref: TranscriptBlockRef,
        role: MessageRole,
        text_digest: TextDigest,
        token_estimate: TokenEstimate,
        replay: ProviderReplayAvailabilityObservation,
        visibility_facts: ImageVisibilityFacts,
        retention_facts: ImageRetentionFacts,
    },
    ImageContent {
        block_ref: TranscriptBlockRef,
        role: MessageRole,
        media: ImageCandidateMediaObservation,
        replay: ProviderReplayAvailabilityObservation,
        visibility_facts: ImageVisibilityFacts,
        retention_facts: ImageRetentionFacts,
    },
    AssistantReasoning {
        block_ref: TranscriptBlockRef,
        token_estimate: TokenEstimate,
        replay: ProviderReplayAvailabilityObservation,
    },
    AssistantToolUse {
        block_ref: TranscriptBlockRef,
        tool_call_id: ToolCallId,
        token_estimate: TokenEstimate,
        adjacency_group: ToolAdjacencyGroupId,
        replay: ProviderReplayAvailabilityObservation,
    },
    ToolResult {
        block_ref: TranscriptBlockRef,
        tool_call_id: ToolCallId,
        token_estimate: TokenEstimate,
        adjacency_group: ToolAdjacencyGroupId,
        content_blocks: Vec<ToolResultContentBlockObservation>,
    },
}

pub struct ToolResultContentBlockObservation {
    pub block_ref: TranscriptBlockRef,
    pub order: ToolResultContentBlockOrder,
    pub content: ProjectionContentBlockObservation,
    pub replay: ProviderReplayAvailabilityObservation,
    pub visibility_facts: ImageVisibilityFacts,
    pub retention_facts: ImageRetentionFacts,
}

pub enum ProjectionContentBlockObservation {
    Text {
        text_digest: TextDigest,
        token_estimate: TokenEstimate,
    },
    Image {
        media: ImageCandidateMediaObservation,
    },
    Video {
        media: VideoCandidateMediaObservation,
    },
    Unsupported {
        media_type: MediaType,
        reason: UnsupportedProjectionContentReason,
    },
}

pub struct ProjectionTokenAccountingSnapshot {
    pub estimator: ProjectionTokenEstimatorRef,
    pub provider_profile_version: ProviderProfileVersion,
    pub token_policy_version: ProjectionTokenPolicyVersion,
    pub rebuild_trigger: ProjectionTokenAccountingRebuildTrigger,
    pub proof_digest: ProjectionTokenAccountingProofDigest,
}

pub struct ProjectionTokenEstimatorRef {
    pub estimator_id: ProjectionTokenEstimatorId,
    pub estimator_version: ProjectionTokenEstimatorVersion,
    pub estimator_digest: ProjectionTokenEstimatorDigest,
}

pub struct TokenEstimate {
    pub count: u32,
    pub estimator: ProjectionTokenEstimatorRef,
    pub source_digest: TokenEstimateSourceDigest,
    pub estimate_digest: TokenEstimateDigest,
}

pub enum ProjectionTokenAccountingRebuildTrigger {
    ProviderProfileChanged,
    EstimatorVersionChanged,
    TranscriptBoundaryChanged,
    CandidateDigestChanged,
}

pub enum ProjectionHistoryObservationValidity {
    Verified { proof: ProjectionHistoryObservationProof },
    Unverifiable { reason: ProjectionObservationUnverifiableReason },
}

pub struct ProjectionHistoryObservationProof {
    pub store_snapshot: StoreSnapshotRef,
    pub transcript_enumeration: TranscriptEnumerationProof,
    pub candidate_enumeration: ProjectionCandidateEnumerationProof,
    pub tool_adjacency: ToolAdjacencyProof,
    pub token_accounting: ProjectionTokenAccountingProofDigest,
    pub compaction_epoch: CompactionEpoch,
    pub visibility_epoch: VisibilityEpoch,
    pub retention_epoch: RetentionEpoch,
    pub replay_availability: ReplayAvailabilityProof,
    pub proof_digest: ProjectionHistoryObservationProofDigest,
}

pub enum ProjectionObservationUnverifiableReason {
    TranscriptEnumerationIncomplete,
    ToolAdjacencyProofMissing,
    TokenAccountingUnverifiable,
    CompactionStateUnavailable,
    VisibilityStateUnavailable,
    RetentionStateUnavailable,
    ReplayAvailabilityUnavailable,
}
```

Tool-result content is intentionally ordered `ContentBlock` observation data,
not a compressed `text/image/text+image` summary. A tool result with multiple
blocks, mixed media, or future visible media must either be projected as ordered
canonical content or denied by provider-consumption preflight; provider lowering
does not get to decide which tool-result blocks survived.

Observation proofs are store/session-owned mechanical products, not shell-built
assertions. The only public constructors are equivalent to:

```rust
build_projection_history_observation(session_id, boundary, request)
build_image_source_resolution_observation(session_id, boundary, inputs)
```

Those constructors enumerate canonical transcript rows, candidate image blocks,
tool-result adjacency groups, compaction state, visibility state, retention
state, replay availability, and token-accounting inputs from one
`StoreSnapshotRef`. The proof digest binds that snapshot id/version plus the
enumeration digests and the resulting observation digest. The catalog machine
accepts `Verified` only when the proof digests match the observation fields and
candidate list; otherwise it records `ProjectionObservationDenied` or the
source-resolution equivalent. Runtime shell code may request observations from
the store, but it must not hand-roll proof objects or downgrade unverifiable
proofs into empty history.

Projection materialization responsibilities:

- materialize the machine-owned `ImageProjectionDecision`;
- resolve selected blob-backed media and record blob ids, digests, and inspected
  media facts such as dimensions/pixel count;
- pass provider adapters a typed, already-selected canonical Meerkat projection.

The projection artifact is explicit:

```rust
pub struct ImageProjectionDecision {
    pub decision_id: ImageProjectionDecisionId,
    pub decision_event_id: MachineEventId,
    pub operation_id: ImageOperationId,
    pub policy_version: ImageProjectionPolicyVersion,
    pub transcript_boundary: TranscriptBoundary,
    pub history_observation: ProjectionHistoryObservationRef,
    pub source_truth_digest: ProjectionSourceTruthDigest,
    pub request_digest: RequestDigest,
    pub provider_plan_ref: ProviderPlanningReceiptRef,
    pub provider_plan_digest: ProviderPlanDigest,
    pub source_resolution_decisions: Vec<ImageSourceResolutionDecisionRef>,
    pub provider_profile_version: ProviderProfileVersion,
    pub provider_projection_capability: ProviderProjectionCapabilitySnapshot,
    pub image_selected_manifest: Vec<ImageProjectionDecisionSelectedItem>,
    pub image_omitted_manifest: Vec<ImageProjectionDecisionOmittedItem>,
    pub canonical_manifest_plan: ProjectionCanonicalManifestPlan,
    pub omitted_canonical_manifest: Vec<ProjectionOmittedCanonicalItem>,
    pub history_policy: ProjectionHistoryPolicySnapshot,
    pub decision_digest: ImageProjectionDecisionDigest,
}

pub struct ImageProjectionDecisionRef {
    pub decision_id: ImageProjectionDecisionId,
    pub decision_event_id: MachineEventId,
    pub decision_digest: ImageProjectionDecisionDigest,
}

pub struct ImageSourceResolutionDecisionRef {
    pub decision_id: MachineDecisionId,
    pub decision_event_id: MachineEventId,
    pub decision_digest: SourceResolutionDecisionDigest,
}

pub struct ProviderProjectionCapabilitySnapshot {
    pub provider: ProviderId,
    pub profile_version: ProviderProfileVersion,
    pub passive_history_channel: PassiveImageHistoryChannel,
    pub max_passive_history_images: u32,
    pub max_passive_history_bytes: u64,
}

pub enum ProjectionSourceRef {
    TranscriptBlock(TranscriptBlockRef),
    RetainedImageAnchor(RetainedImageAnchorId),
}

pub enum PassiveImageHistoryChannel {
    None,
    ProviderNativeHistory {
        proof: PassiveHistorySemanticsProof,
    },
    PurposePreservingAdaptedHistory {
        proof: PassiveHistorySemanticsProof,
    },
}

pub struct PassiveHistorySemanticsProof {
    pub proof_id: ProviderCapabilityProofId,
    pub provider_profile_version: ProviderProfileVersion,
    pub guarantees: Vec<PassiveHistoryGuarantee>,
}

pub enum PassiveHistoryGuarantee {
    ContextOnly,
    NotEditSource,
    NotGenerationReference,
}

pub enum ImageProjectionDecisionSelectedItem {
    AssistantImage {
        selection_id: ProjectionSelectionItemId,
        image_id: AssistantImageId,
        source_ref: ProjectionSourceRef,
        purpose: ImageProjectionPurpose,
        replay: ProviderReplayRef,
    },
    TranscriptImage {
        selection_id: ProjectionSelectionItemId,
        source_ref: ProjectionSourceRef,
        purpose: ImageProjectionPurpose,
        replay: ProviderReplayRef,
    },
}

pub struct ImageProjectionDecisionOmittedItem {
    pub source_ref: ProjectionSourceRef,
    pub reason: ImageProjectionOmittedReason,
    pub replay: ProviderReplayRef,
}

pub struct ImageProjectionSnapshot {
    pub projection_snapshot_id: ProjectionSnapshotId,
    pub decision_id: ImageProjectionDecisionId,
    pub decision_digest: ImageProjectionDecisionDigest,
    pub operation_id: ImageOperationId,
    pub policy_version: ImageProjectionPolicyVersion,
    pub transcript_boundary: TranscriptBoundary,
    pub source_truth_digest: ProjectionSourceTruthDigest,
    pub request_digest: RequestDigest,
    pub provider_plan_digest: ProviderPlanDigest,
    pub selected: Vec<ImageProjectionSelectedItem>,
    pub omitted: Vec<ImageProjectionOmittedItem>,
    pub canonical_manifest: ProjectionCanonicalManifest,
}

pub enum ImageProjectionSelectedItem {
    AssistantImage {
        selection_id: ProjectionSelectionItemId,
        image_id: AssistantImageId,
        source_ref: ProjectionSourceRef,
        purpose: ImageProjectionPurpose,
        media: SelectedImageMedia,
        replay: ProviderReplayRef,
    },
    TranscriptImage {
        selection_id: ProjectionSelectionItemId,
        source_ref: ProjectionSourceRef,
        purpose: ImageProjectionPurpose,
        media: SelectedImageMedia,
        replay: ProviderReplayRef,
    },
}

pub enum ImageProjectionPurpose {
    PassiveHistoryContext,
    EditSource,
    GenerationReference,
}

pub struct SelectedImageMedia {
    pub blob_id: BlobId,
    pub media_type: MediaType,
    pub byte_sha256: String,
    pub byte_len: u64,
    pub inspected: InspectedImageMediaFacts,
    pub source_origin: ImageCandidateSourceOrigin,
}

pub struct InspectedImageMediaFacts {
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub pixel_count: Option<u64>,
    pub frame_count: Option<u32>,
    pub color_space: Option<ImageColorSpace>,
    pub inspection_digest: MediaInspectionDigest,
}

pub struct ProjectionCanonicalManifest {
    pub items: Vec<ProjectedCanonicalItem>,
    pub replay_refs: Vec<ProjectedReplayRef>,
}

pub struct ProjectionCanonicalManifestPlan {
    pub items: Vec<ProjectedCanonicalItemPlan>,
    pub replay_refs: Vec<ProjectedReplayRef>,
}

pub struct ProjectionHistoryPolicySnapshot {
    pub policy_version: ImageProjectionPolicyVersion,
    pub transcript_boundary: TranscriptBoundary,
    pub token_accounting: ProjectionTokenAccountingSnapshot,
    pub max_text_tokens: u32,
    pub max_reasoning_tokens: u32,
    pub include_tool_calls: bool,
    pub include_reasoning: bool,
    pub truncation_strategy: ProjectionTruncationStrategy,
    pub policy_digest: ProjectionHistoryPolicyDigest,
}

pub enum ProjectionTruncationStrategy {
    NewestFirst,
    PreserveToolAdjacency,
    PreserveExplicitSources,
}

pub enum ProjectedCanonicalItemPlan {
    TextBlock {
        message_ref: TranscriptBlockRef,
        role: MessageRole,
        text_digest: TextDigest,
        replay: ProviderReplayRef,
    },
    SelectedImageBlock {
        selection_id: ProjectionSelectionItemId,
        source_ref: ProjectionSourceRef,
        purpose: ImageProjectionPurpose,
        replay: ProviderReplayRef,
    },
    ToolUseBlock {
        message_ref: TranscriptBlockRef,
        tool_call_id: ToolCallId,
        replay: ProviderReplayRef,
    },
    ToolResultBlock {
        message_ref: TranscriptBlockRef,
        tool_call_id: ToolCallId,
        content_blocks: Vec<ToolResultContentBlockPlan>,
        replay: ProviderReplayRef,
    },
    ReasoningBlock {
        message_ref: TranscriptBlockRef,
        replay: ProviderReplayRef,
    },
}

pub struct ProjectionOmittedCanonicalItem {
    pub item_ref: ProjectionCanonicalItemRef,
    pub reason: ProjectionCanonicalOmittedReason,
    pub policy_decision: MachineDecisionId,
}

pub enum ProjectionCanonicalItemRef {
    TextBlock {
        message_ref: TranscriptBlockRef,
    },
    SelectedImageBlock {
        selection_id: ProjectionSelectionItemId,
        source_ref: ProjectionSourceRef,
        purpose: ImageProjectionPurpose,
    },
    ToolUseBlock {
        message_ref: TranscriptBlockRef,
        tool_call_id: ToolCallId,
    },
    ToolResultBlock {
        message_ref: TranscriptBlockRef,
        tool_call_id: ToolCallId,
    },
    ReasoningBlock {
        message_ref: TranscriptBlockRef,
    },
}

pub enum ProjectionCanonicalOmittedReason {
    OutsideWindow,
    TokenBudget,
    Compacted,
    ProviderUnsupported,
    ToolAdjacencyGuard,
    ReasoningPolicy,
    ReplayNotRequired,
}

pub enum ProjectedCanonicalItem {
    TextBlock {
        message_ref: TranscriptBlockRef,
        role: MessageRole,
        text_digest: TextDigest,
        replay: ProviderReplayRef,
    },
    SelectedImageBlock {
        selected_index: u32,
        source_ref: ProjectionSourceRef,
        purpose: ImageProjectionPurpose,
        blob_id: BlobId,
        byte_sha256: String,
        inspected: InspectedImageMediaFacts,
        replay: ProviderReplayRef,
    },
    ToolUseBlock {
        message_ref: TranscriptBlockRef,
        tool_call_id: ToolCallId,
        replay: ProviderReplayRef,
    },
    ToolResultBlock {
        message_ref: TranscriptBlockRef,
        tool_call_id: ToolCallId,
        content_blocks: Vec<ProjectedToolResultContentBlock>,
        replay: ProviderReplayRef,
    },
    ReasoningBlock {
        message_ref: TranscriptBlockRef,
        replay: ProviderReplayRef,
    },
}

pub struct ToolResultContentBlockPlan {
    pub block_ref: TranscriptBlockRef,
    pub order: ToolResultContentBlockOrder,
    pub content: ProjectionContentBlockObservation,
    pub replay: ProviderReplayRef,
}

pub struct ProjectedToolResultContentBlock {
    pub block_ref: TranscriptBlockRef,
    pub order: ToolResultContentBlockOrder,
    pub content: ProjectedContentBlock,
    pub replay: ProviderReplayRef,
}

pub enum ProjectedContentBlock {
    Text {
        text_digest: TextDigest,
    },
    Image {
        blob_id: BlobId,
        byte_sha256: String,
        inspected: InspectedImageMediaFacts,
    },
    Video {
        blob_id: BlobId,
        byte_sha256: String,
        inspected: InspectedVideoMediaFacts,
    },
}

pub struct ProjectedReplayRef {
    pub owner: ProviderReplayOwnerRef,
    pub replay: ProviderReplayRef,
}

pub struct ImageProjectionOmittedItem {
    pub source_ref: ProjectionSourceRef,
    pub reason: ImageProjectionOmittedReason,
    pub replay: ProviderReplayRef,
}

pub enum ImageProjectionOmittedReason {
    OutsideWindow,
    Compacted,
    AlreadySelectedAsSourceOrReference,
    PassiveHistoryLimit,
    PassiveHistoryByteBudget,
    ProviderHasNoPassiveHistoryChannel,
}

pub enum SubstrateMediaProjectionPolicy {
    SupportedMediaTypes {
        media_types: Vec<MediaType>,
        policy_version: ImageProjectionPolicyVersion,
    },
    MaxHydrationBytes {
        limit: u64,
        policy_version: ImageProjectionPolicyVersion,
    },
}

pub enum ImageProjectionFailure {
    MissingSelectedBlob {
        source_ref: ProjectionSourceRef,
        transcript_boundary: TranscriptBoundary,
        selected_item: SelectedProjectionItemRef,
        expected_blob: BlobId,
    },
    UnsupportedSelectedMediaType {
        source_ref: ProjectionSourceRef,
        selected_item: SelectedProjectionItemRef,
        media_type: MediaType,
        policy: SubstrateMediaProjectionPolicy,
    },
    OversizedSelectedMedia {
        source_ref: ProjectionSourceRef,
        selected_item: SelectedProjectionItemRef,
        byte_len: u64,
        limit: u64,
        policy: SubstrateMediaProjectionPolicy,
    },
    ReplayMaterialLost {
        selected_item: SelectedProjectionItemRef,
        replay_owner: ProviderReplayOwnerRef,
        decision_id: ReplayMaterialLostDecisionId,
        reason: ProviderReplayMissingReason,
    },
    UnverifiableProjectionObservation {
        observation: ProjectionHistoryObservationRef,
        reason: ProjectionObservationUnverifiableReason,
    },
}

pub enum ImageSourceResolutionDenial {
    MixedConcreteAndSelector {
        set: ImageSourceSetKind,
    },
    NoVisibleAssistantImage {
        boundary: LatestVisibleImageBoundary,
    },
    SourceNotVisible {
        source_ref: ImageSourceRef,
    },
    RetainedAnchorUnavailable {
        anchor_id: RetainedImageAnchorId,
    },
    CandidateMediaUnavailable {
        source_ref: ConcreteImageSourceRef,
        reason: ImageCandidateMediaUnavailableReason,
    },
    CandidateBlobMissing {
        source_ref: ConcreteImageSourceRef,
        blob_id: BlobId,
    },
    CandidateBlobUnreachable {
        source_ref: ConcreteImageSourceRef,
        blob_id: BlobId,
    },
    SurfaceImageHandleNotReachable {
        handle: SurfaceImageHandle,
    },
    SurfaceImageHandleMediaMismatch {
        handle: SurfaceImageHandle,
        declared_media_type: MediaType,
        observed_media_type: MediaType,
    },
    UnverifiableObservation {
        reason: SourceObservationUnverifiableReason,
    },
}

pub enum ProviderConsumptionPreflight {
    Approved {
        plan: ProviderConsumptionPlan,
        provider_profile_version: ProviderProfileVersion,
        input: ProviderConsumptionPreflightInputRef,
        approval_manifest: ProviderConsumptionApprovalManifest,
    },
    Denied {
        reason: ProviderConsumptionDenial,
        provider_profile_version: ProviderProfileVersion,
        input: ProviderConsumptionPreflightInputRef,
    },
}

pub struct ProviderConsumptionPreflightInputRef {
    pub input_id: ProviderConsumptionPreflightInputId,
    pub input_digest: ProviderConsumptionPreflightInputDigest,
}

pub struct ProviderConsumptionPreflightInput {
    pub input_id: ProviderConsumptionPreflightInputId,
    pub projection_decision: ImageProjectionDecisionRef,
    pub provider: ProviderId,
    pub target_model: ModelId,
    pub provider_profile_version: ProviderProfileVersion,
    pub planning_receipt: ProviderPlanningReceiptRef,
    pub provider_plan_digest: ProviderPlanDigest,
    pub canonical_manifest: ProjectionCanonicalManifest,
    pub selected_media: Vec<ImageProjectionSelectedItem>,
    pub input_digest: ProviderConsumptionPreflightInputDigest,
}

pub struct ProviderConsumptionPreflightReceiptRef {
    pub receipt_id: ProviderConsumptionPreflightReceiptId,
    pub input: ProviderConsumptionPreflightInputRef,
    pub provider_profile_version: ProviderProfileVersion,
    pub evidence_digest: ProviderConsumptionEvidenceDigest,
}

pub struct ProviderConsumptionPreflightReceipt {
    pub receipt_id: ProviderConsumptionPreflightReceiptId,
    pub input: ProviderConsumptionPreflightInputRef,
    pub provider_profile_version: ProviderProfileVersion,
    pub outcome: ProviderConsumptionPreflightReceiptOutcome,
    pub evidence_digest: ProviderConsumptionEvidenceDigest,
}

pub enum ProviderConsumptionPreflightReceiptOutcome {
    Approved {
        plan: ProviderConsumptionPlanRef,
        approval_manifest: ProviderConsumptionApprovalManifest,
    },
    Denied {
        reason: ProviderConsumptionDenial,
    },
}

pub struct ProviderConsumptionApprovalManifest {
    pub input: ProviderConsumptionPreflightInputRef,
    pub canonical_manifest_digest: ProjectionCanonicalManifestDigest,
    pub item_approvals: Vec<ProviderConsumptionItemApproval>,
    pub replay_approvals: Vec<ProviderReplayConsumptionApproval>,
    pub adjacency_approvals: Vec<ProviderToolAdjacencyApproval>,
    pub approval_digest: ProviderConsumptionApprovalDigest,
}

pub struct ProviderConsumptionItemApproval {
    pub item: ProjectionCanonicalItemRef,
    pub semantics: ProviderCanonicalItemConsumptionSemantics,
    pub provider_profile_version: ProviderProfileVersion,
}

pub enum ProviderCanonicalItemConsumptionSemantics {
    Text(ProviderTextInputSemantics),
    Image {
        purpose: ImageProjectionPurpose,
        semantics: ProviderImageInputSemantics,
    },
    Video(ProviderVideoInputSemantics),
    Reasoning(ProviderReasoningReplaySemantics),
    ToolUse(ProviderToolUseSemantics),
    ToolResult(ProviderToolResultSemantics),
}

pub struct ProviderReplayConsumptionApproval {
    pub replay: ProjectedReplayRef,
    pub semantics: ProviderReplaySemantics,
}

pub struct ProviderToolAdjacencyApproval {
    pub group: ToolAdjacencyGroupId,
    pub semantics: ProviderToolAdjacencySemantics,
}

pub enum ProviderImageInputSemantics {
    PassiveContextPreserved,
    EditSource,
    GenerationReference,
}

pub enum ProviderTextInputSemantics {
    NativeText,
    PromptInstructionText,
}

pub enum ProviderVideoInputSemantics {
    NativeVideo,
}

pub enum ProviderReasoningReplaySemantics {
    NativeReasoningReplay,
}

pub enum ProviderToolUseSemantics {
    NativeToolCall,
}

pub enum ProviderToolResultSemantics {
    NativeToolResult,
    ContentOnlyToolResult,
}

pub enum ProviderReplaySemantics {
    NativeReplayMaterial,
    NotRequired,
}

pub enum ProviderToolAdjacencySemantics {
    Preserved,
}

pub enum ProjectedCanonicalItemRef {
    TranscriptBlock(TranscriptBlockRef),
    ToolAdjacencyGroup(ToolAdjacencyGroupId),
    ReplayRef(ProjectedReplayRefId),
}

pub enum ProviderConsumptionDenial {
    UnsupportedMediaType {
        selected_item: SelectedProjectionItemRef,
        media_type: MediaType,
    },
    OversizedMedia {
        selected_item: SelectedProjectionItemRef,
        byte_len: u64,
        limit: u64,
    },
    UnsupportedRoleAdaptation {
        selected_item: SelectedProjectionItemRef,
        original_message_role: MessageRole,
    },
    UnsupportedPurposeSemantics {
        selected_item: SelectedProjectionItemRef,
        purpose: ImageProjectionPurpose,
    },
    UnsupportedTextContent {
        item: ProjectedCanonicalItemRef,
        reason: ProviderTextConsumptionDenialReason,
    },
    UnsupportedReasoningReplay {
        item: ProjectedCanonicalItemRef,
        reason: ProviderReasoningConsumptionDenialReason,
    },
    UnsupportedToolUse {
        item: ProjectedCanonicalItemRef,
        reason: ProviderToolUseConsumptionDenialReason,
    },
    UnsupportedToolResult {
        item: ProjectedCanonicalItemRef,
        reason: ProviderToolResultConsumptionDenialReason,
    },
    UnsupportedToolAdjacency {
        group: ToolAdjacencyGroupId,
        reason: ProviderToolAdjacencyDenialReason,
    },
    UnsupportedReplayRequirement {
        replay: ProjectedReplayRef,
        reason: ProviderReplayConsumptionDenialReason,
    },
    UnsupportedCanonicalItem {
        item: ProjectedCanonicalItemRef,
        reason: ProviderCanonicalItemDenialReason,
    },
}
```

`ProviderConsumptionApprovalManifest` is the typed, machine-checkable provider
consumption truth for the whole canonical manifest. A provider profile must
either bind each selected image to provider semantics that preserve
`ImageProjectionPurpose`, bind every selected text/reasoning/tool/tool-result
item, replay ref, and tool-adjacency group to provider-consumable semantics, or
deny with a typed `ProviderConsumptionDenial`. The opaque
`ProviderConsumptionPlanEnvelope` is provider serialization material only; it is
invalid unless its digest is paired with an approval manifest digest for the same
preflight input and canonical manifest. Provider request builders are not
allowed to discover the first semantic "provider cannot consume this selected
history" error during lowering. Approval manifests contain only positive
bindings; unsupported text, media, reasoning, tool, replay, or adjacency facts
must be represented as `ProviderConsumptionDenial`, never as an "approved but
unsupported" item.
Both non-`None` passive history channels must carry
`PassiveHistorySemanticsProof` with `ContextOnly`, `NotEditSource`, and
`NotGenerationReference` guarantees. The adapted-history variant is only legal
when that proof shows the adapted channel keeps selected images as passive
context. Backends where every supplied image is semantically an edit
source/reference declare `PassiveImageHistoryChannel::None` and cannot consume
`PassiveHistoryContext` images through that channel.

`ImageProjectionDecision.canonical_manifest_plan` is the machine-owned selected
provider-history manifest for text, image, tool-use, and reasoning blocks.
`ImageProjectionDecision.omitted_canonical_manifest` is the matching
machine-owned omitted manifest with typed reasons for token/window truncation,
compaction, provider support, tool-adjacency guards, and reasoning policy.
Projection code materializes that plan; it does not choose provider-history
membership. `ImageProjectionSnapshot.canonical_manifest` is the realized
projection after materialization. It stores explicit provider-visible block
items, selected blob ids, byte/media-inspection digests, policy versions,
request/plan digests, and replay bindings. It must not store hydrated inline
image bytes and must not store full request or provider-plan payloads. The
hydrated provider execution view is built just-in-time from those explicit block
items plus blob/source truth and validated against digests before provider
lowering. The snapshot is not provider wire shape and it is not role-adapted.
The snapshot's selected/omitted manifest is a copy of the machine-owned
`ImageProjectionDecision` for audit evidence and determinism checks. Rebuilding
the same decision over the same transcript boundary and blob store should
produce the same selected set, or fail with a typed projection mismatch.

`ImageProjectionSnapshot` exists only for successful projection. Benign
non-selection facts, such as content outside the window, compacted away, or not
needing a replay envelope, are `ImageProjectionOmittedReason`s. Selector and
visibility failures, such as mixed selectors, no visible image, invisible
selected content, unreachable retained anchors, or invalid surface image handles,
are `ImageSourceResolutionDenial`s. Selected-content materialization failures,
such as missing selected blobs, unsupported media under Meerkat substrate
hydration policy, oversized media under Meerkat substrate hydration policy, or
selected replay material lost, are `ImageProjectionFailure`s. They carry
`SubstrateMediaProjectionPolicy` when the denial is about local media type or byte
limits. They are machine-owned terminal/denial facts and are not recorded as mere
omissions in a successful snapshot. Provider-specific media type, size, role, and
purpose limits are `ProviderConsumptionDenial`s and always carry a required
`ProviderProfileVersion` through `ProviderConsumptionPreflight`.

Provider media-consumption legality is not decided during provider lowering.
Before snapshot binding, projection materialization inspects selected media,
records `InspectedImageMediaFacts` in `SelectedImageMedia`, writes a durable
`ProviderConsumptionPreflightInput`, and reports that input ref to the machine as
the staged projection attempt evidence. The provider profile then runs preflight
over that immutable input and returns `ProviderConsumptionPreflight`. For an
approval, the shell first persists the opaque `ProviderConsumptionPlanReceipt`
bound to the typed approval manifest digest, then persists a
`ProviderConsumptionPreflightReceipt` that carries the approval manifest and
points at that plan; for a denial, the receipt stores the typed denial evidence.
The machine observes only the durable receipt. If denied, the machine
terminalizes through its
provider-consumption denial path before any snapshot is bound or provider
request is serialized, and recovery reloads the same preflight input and receipt
by digest. Provider request builders may still fail on mechanical transport or
serialization defects, but they must not be the first semantic owner of
"provider cannot consume selected media."

Snapshot persistence is its own machine-observed step. After materialization
writes `ProviderConsumptionPreflightInput` and before provider preflight can run,
the shell writes `ImageProjectionSnapshot` with the same manifest digest and
reports either the snapshot id/digest or a store error digest to the machine. A
snapshot write failure terminalizes or retries through
`ProjectionSnapshotPersistFailed`; it is not folded into generic projection
denial and it is not retried from runtime memory. Snapshot binding is legal only
after the machine has observed the durable snapshot receipt and the later
preflight approval receipt.

Projection snapshots live in session-scoped projection snapshot storage owned by
the session store:

```rust
put_image_projection_snapshot(session_id, snapshot)
get_image_projection_snapshot(session_id, projection_snapshot_id)
list_image_projection_snapshots(session_id, operation_id)
put_projection_history_observation(session_id, observation)
get_projection_history_observation(session_id, observation_id)
put_provider_consumption_preflight_input(session_id, input)
get_provider_consumption_preflight_input(session_id, input_id)
put_provider_consumption_plan_receipt(session_id, receipt)
get_provider_consumption_plan_receipt(session_id, receipt_id)
put_provider_consumption_preflight_receipt(session_id, receipt)
get_provider_consumption_preflight_receipt(session_id, receipt_id)
```

For an approved preflight, the store/session layer commits
`ProviderConsumptionPlanReceipt` and `ProviderConsumptionPreflightReceipt` in one
receipt transaction. A preflight receipt that names a missing or digest-mismatched
plan receipt is invalid and terminalizes through provider-consumption plan
mismatch; runtime recovery must not recreate the opaque provider plan envelope.

`ImageProjectionDecision` is not a cache row. It is a durable generated machine
event payload and state entry. Recovery loads it from the machine event log using
`ImageProjectionDecisionRef`; it must not recompute selected/omitted membership
from current projection code. If the decision payload named by the ref is absent
or digest-mismatched, recovery reports machine-event corruption instead of
rebuilding policy in the shell. The decision points at the
`ProjectionHistoryObservationRef` that supplied canonical candidates; recovery
reloads that observation by digest before rebuilding any projection evidence.
If the observation is `Unverifiable`, the machine records
`ProjectionObservationDenied` and terminalizes through
`ImageProjectionFailure::UnverifiableProjectionObservation`; shell code must not
reinterpret that condition as an empty history window or a generic projection
mismatch.
Any provider-profile facts that influenced selection, especially passive history
capability and limits, are copied into the decision as
`ProviderProjectionCapabilitySnapshot`; later provider profile changes do not
reinterpret an existing decision.

The machine stores a projection binding record, not just an aggregate hash:

```rust
pub struct ProjectionAttemptRecord {
    pub projection_attempt_id: ProjectionAttemptId,
    pub operation_id: ImageOperationId,
    pub projection_decision: ImageProjectionDecisionRef,
    pub source_truth_digest: ProjectionSourceTruthDigest,
    pub transcript_boundary: TranscriptBoundary,
    pub request_digest: RequestDigest,
    pub provider_params_digest: ProviderParamsDigest,
    pub provider_plan_ref: ProviderPlanningReceiptRef,
    pub provider_plan_digest: ProviderPlanDigest,
    pub provider_profile_version: ProviderProfileVersion,
    pub projection_policy_version: ImageProjectionPolicyVersion,
    pub outcome: ProjectionAttemptOutcome,
}

pub enum ProjectionAttemptOutcome {
    ProjectionObservationDenied {
        observation: ProjectionHistoryObservationRef,
        reason: ProjectionObservationUnverifiableReason,
        evidence_digest: ProjectionFailureEvidenceDigest,
    },
    PreflightInputStaged {
        preflight_input: ProviderConsumptionPreflightInputRef,
        projection_snapshot_id: ProjectionSnapshotId,
        projection_snapshot_digest: ProjectionSnapshotDigest,
    },
    ProjectionSnapshotPersistFailed {
        preflight_input: ProviderConsumptionPreflightInputRef,
        error_digest: ProjectionSnapshotStoreErrorDigest,
    },
    SnapshotBound {
        projection_snapshot_id: ProjectionSnapshotId,
        preflight_input: ProviderConsumptionPreflightInputRef,
        preflight_receipt: ProviderConsumptionPreflightReceiptRef,
        approved_consumption_plan: ProviderConsumptionPlanRef,
        selected_blob_digests: Vec<BlobDigest>,
    },
    ProjectionDenied {
        failure: ImageProjectionFailure,
        evidence_digest: ProjectionFailureEvidenceDigest,
    },
    ProviderConsumptionDenied {
        reason: ProviderConsumptionDenial,
        provider_profile_version: ProviderProfileVersion,
        preflight_input: ProviderConsumptionPreflightInputRef,
        preflight_receipt: ProviderConsumptionPreflightReceiptRef,
        evidence_digest: ProviderConsumptionEvidenceDigest,
    },
    StaleSourceResolution {
        decision_ids: Vec<MachineDecisionId>,
        observed_digest: SourceResolutionObservationDigest,
    },
}

pub struct ProviderConsumptionPlanRef {
    pub receipt_id: ProviderConsumptionPlanReceiptId,
    pub provider: ProviderId,
    pub target_model: ModelId,
    pub provider_profile_version: ProviderProfileVersion,
    pub planning_receipt: ProviderPlanningReceiptRef,
    pub preflight_input: ProviderConsumptionPreflightInputRef,
    pub approval_manifest_digest: ProviderConsumptionApprovalDigest,
    pub plan_digest: ProviderConsumptionPlanDigest,
    pub plan_envelope_schema: ProviderConsumptionPlanEnvelopeSchema,
    pub plan_envelope_schema_version: ProviderConsumptionPlanEnvelopeSchemaVersion,
}

pub struct ProviderPlanningReceiptRef {
    pub receipt_id: ProviderPlanningReceiptId,
    pub provider: ProviderId,
    pub target_model: ModelId,
    pub provider_profile_version: ProviderProfileVersion,
    pub plan_envelope_schema: ProviderPlanEnvelopeSchema,
    pub plan_envelope_schema_version: ProviderPlanEnvelopeSchemaVersion,
    pub provider_plan_digest: ProviderPlanDigest,
    pub plan_envelope_digest: ProviderPlanEnvelopeDigest,
}

pub struct ProviderPlanningReceipt {
    pub receipt_id: ProviderPlanningReceiptId,
    pub provider: ProviderId,
    pub provider_profile_version: ProviderProfileVersion,
    pub target_model: ModelId,
    pub scoped_execution: ImageScopedExecutionRequirement,
    pub plan_envelope_schema: ProviderPlanEnvelopeSchema,
    pub plan_envelope_schema_version: ProviderPlanEnvelopeSchemaVersion,
    pub plan_digest: ProviderPlanDigest,
    pub plan_envelope_digest: ProviderPlanEnvelopeDigest,
    pub plan_envelope: ProviderImageExecutionPlanEnvelope,
}

pub struct ProviderParamsDenialReceipt {
    pub receipt_id: ProviderParamsDenialReceiptId,
    pub provider: ProviderId,
    pub schema: ProviderImageParamsSchema,
    pub schema_version: ProviderImageParamsSchemaVersion,
    pub evidence_digest: ProviderParamsDenialEvidenceDigest,
}

pub struct ProviderPlanningDenialReceipt {
    pub receipt_id: ProviderPlanningDenialReceiptId,
    pub provider: ProviderId,
    pub provider_profile_version: ProviderProfileVersion,
    pub target_resolution: ProviderPlanningTargetResolution,
    pub evidence_digest: ProviderPlanningDenialEvidenceDigest,
}

pub enum ProviderPlanningTargetResolution {
    UsesSessionModel,
    ExplicitTarget { model: ModelId },
    InheritedDefault { model: ModelId },
    TargetResolutionFailed { reason: ProviderTargetResolutionFailure },
}

pub struct ProviderConsumptionPlanReceipt {
    pub receipt_id: ProviderConsumptionPlanReceiptId,
    pub provider: ProviderId,
    pub target_model: ModelId,
    pub provider_profile_version: ProviderProfileVersion,
    pub planning_receipt: ProviderPlanningReceiptRef,
    pub preflight_input: ProviderConsumptionPreflightInputRef,
    pub approval_manifest_digest: ProviderConsumptionApprovalDigest,
    pub plan_digest: ProviderConsumptionPlanDigest,
    pub plan_envelope_schema: ProviderConsumptionPlanEnvelopeSchema,
    pub plan_envelope_schema_version: ProviderConsumptionPlanEnvelopeSchemaVersion,
    pub plan_envelope: ProviderConsumptionPlanEnvelope,
}
```

The machine records a `ProjectionAttemptRecord` for every projection/preflight
attempt, not only for successful snapshots. A successful outcome points at an
`ImageProjectionSnapshot` and an approved provider consumption plan ref. The
opaque approved plan envelope lives in provider/session sidecar storage as
`ProviderConsumptionPlanReceipt` and is loaded by receipt id when a provider
request must be serialized. The receipt is self-binding: it names the provider,
target model, provider profile version, accepted planning receipt, preflight
input, typed approval manifest digest, envelope schema/version, and plan digest,
so recovery cannot accidentally load a stale or cross-provider consumption plan.
Denied outcomes carry the typed
machine terminal reason plus enough digest evidence to audit the attempt against
the durable `ImageProjectionDecision`. The snapshot row is rebuildable
evidence/cache, not authority. The attempt record's source truth is explicitly:

- machine state/event log at the operation phase;
- canonical transcript at `transcript_boundary`;
- `GenerateImageRequest` tool arguments, including typed selectors;
- `ImageProjectionDecisionRef`, loaded from generated machine event history;
- `ProjectionHistoryObservationRef`, loaded from the durable observation store;
- `ProviderProjectionCapabilitySnapshot` embedded in that decision;
- provider params envelope digest;
- approved provider planning receipt id and resolved provider plan envelope
  digest;
- provider profile version;
- projection policy version;
- preflight input id and digest after selected media has been materialized;
- preflight receipt id, profile version, and evidence digest after provider
  preflight has approved or denied the immutable input;
- approved provider consumption plan receipt id and digest only after a
  successful snapshot-bound outcome;
- selected blob ids and byte digests only after a successful snapshot-bound
  outcome.

Recovery policy:

- recover machine state from machine events/snapshots first;
- for a planned image operation, reload the machine-observed
  `ProviderPlanningReceipt` or planning-denial receipt by id and digest before
  source resolution, projection, or scoped override recovery;
- for an in-flight image operation with a projection attempt, load the
  `ProjectionAttemptRecord`, then load the referenced `ImageProjectionDecision`
  from machine event history and validate its digest;
- load the referenced `ProjectionHistoryObservation` and validate that its
  transcript boundary and digest match the decision before rebuilding snapshot
  evidence;
- for `PreflightInputStaged`, reload the durable
  `ProviderConsumptionPreflightInput`, validate its provider, target model,
  provider profile version, planning receipt, provider plan digest, projection
  decision, canonical manifest digest, approval manifest digest when present,
  observed projection snapshot digest, and input digest against the machine
  attempt, then either ask that exact provider profile version to produce a fresh
  receipt or terminalize with provider-consumption-preflight-evaluator-unavailable;
- for a snapshot-bound outcome, load the preflight input, preflight receipt, and
  snapshot row if present, then validate the manifest, source truth digest,
  stored `ProviderConsumptionPlanReceipt`, and selected blob digests against the
  machine-owned `ImageProjectionDecision`, typed approval manifest, and
  `ProjectionAttemptRecord`;
- load the sidecar `ProviderConsumptionPlanReceipt` by the machine-bound receipt
  id and validate its digest and approval manifest digest before provider
  execution or recovery; if the plan receipt, preflight receipt, approval
  manifest, or input is missing or digest-mismatched,
  terminalize through the machine's provider-consumption-plan-mismatch class
  rather than silently switching plans;
- if the snapshot row is missing or validation fails, rebuild it from source
  truth and the loaded `ImageProjectionDecision`, then compare the rebuilt
  component digests to the `ProjectionAttemptRecord`;
- if a loaded snapshot or rebuilt snapshot mismatches, terminalize through the
  machine's projection mismatch class;
- for projection-denied, provider-consumption-denied, or stale-source-resolution
  outcomes, rebuild only verification evidence from source truth and the loaded
  `ImageProjectionDecision`, then compare digests before re-emitting the machine
  terminal class;
- for provider-consumption-denied outcomes, reload the durable
  `ProviderConsumptionPreflightInput` and
  `ProviderConsumptionPreflightReceipt`, then compare their digests before
  re-emitting the terminal class;
- never recover operation truth from the projection snapshot row alone.

Retention policy:

- keep projection snapshots at least until their image operation is terminal and
  the paired audit record is durably written;
- retain terminal-operation snapshots while any reachable audit record or
  generated image references them;
- after retention expiry, rebuilding the projection for diagnostics is allowed
  only from source truth and must be marked as reconstructed evidence.

Provider responsibilities after preflight approval:

- lower `ContentBlock::Image` into provider-native image input shape;
- lower `AssistantBlock::ToolUse` into provider-native tool-call shape;
- lower `AssistantBlock::Reasoning` and internal `ProviderReplayAttachment`s
  according to provider replay rules;
- serialize the already-approved consumption plan. Request builders may fail for
  mechanical transport/serialization defects, but not for semantic provider
  media-consumption denial.

Examples:

- Gemini lowers selected image content to `inlineData` parts where legal.
- OpenAI lowers selected image content to `input_image` parts or image-edit
  references where legal for the chosen backend.
- Anthropic lowers selected image content to Anthropic image source blocks.

Provider crates own role/schema adaptation. For example, if a provider cannot
represent an assistant-role image directly, the provider adapter can lower the
selected canonical image as a legal reference/input image part while preserving
the Meerkat transcript meaning in provider-local lowering evidence.

The provider crate may not silently invent history policy. It may also not emit
empty assistant/model messages for selected content. Semantic inability to
consume selected media must be reported by provider-profile preflight before
snapshot binding, not discovered first during lowering.

Provider lowering evidence is provider-owned and appended to the audit record as
`ProviderTraceEvidence` plus replay attachment digests. Replay attachments are
the sole source for future provider replay material; audit trace evidence is not
used for replay recovery. It is not stored as `projected_role` or
`lowered_as_hint` in the shared projection snapshot.

### `generate_image` in the target model

The privileged `generate_image` flow remains. What changes is the transcript
commit shape.

Target flow:

1. the active model calls `generate_image`;
2. the machine begins an image operation;
3. the provider planner resolves an execution plan, the shell persists an
   approved `ProviderPlanningReceipt` or denied planning receipt, and the machine
   observes the durable receipt;
4. the shell supplies source-resolution observations and the complete
   `ProjectionHistoryObservation` for the request boundary, then the machine
   emits source-resolution decisions;
5. the machine emits an `ImageProjectionDecision` from that observation,
   containing the selected item manifest, omitted item manifest, ordering, replay
   bindings, and truncation / compaction decisions;
6. the projection materializer materializes and verifies that exact decision into
   a canonical manifest, writes a durable `ProviderConsumptionPreflightInput` and
   `ImageProjectionSnapshot`, and reports those durable refs to the machine;
   snapshot persistence failure is reported as
   `ProjectionSnapshotPersistFailed`, while typed projection failures are
   submitted to the machine instead;
7. the machine records a `ProjectionAttemptRecord` in `PreflightInputStaged` with
   the observed snapshot ref or a typed projection-denied/snapshot-persist-failed
   outcome before provider media-consumption preflight runs;
8. when a verified manifest exists, the provider profile preflights the immutable
   input for media-consumption legality; on approval it returns a typed approval
   manifest plus opaque provider plan envelope, the shell persists the
   `ProviderConsumptionPlanReceipt` bound to that manifest and then the
   `ProviderConsumptionPreflightReceipt`, while on denial it persists the typed
   denial receipt; the machine observes the durable receipt;
9. if preflight succeeds, the machine binds the stored `ImageProjectionSnapshot`
   and approved `ProviderConsumptionPlanReceipt`; if preflight denies, the
   attempt outcome carries the typed provider-consumption reason and receipt;
10. if the plan's `ImageScopedExecutionRequirement` requires a scoped image-model
   target, the machine/runtime seam binds the current restore target, topology
   epoch, and fence, then the machine applies the operation-scoped override;
   apply failure terminalizes typed;
11. the provider executor serializes the approved consumption plan into a durable
   `ProviderDispatchEnvelopeReceipt`; serialization failure terminalizes typed
   before any external provider attempt exists;
12. the shell durably stages a `ProviderCallAttemptReceipt` with the provider
   idempotency/correlation key and dispatch envelope digest, then sends the
   provider request;
13. if the provider returns, the shell durably stages a `ProviderOutputReceipt`
    with image bytes and provider metadata;
14. any operation-scoped model override is restored;
15. the blob store commits image bytes and the machine records committed image
   receipts;
16. each tool call contributes a `ToolBatchParticipantCommitPart` to the current
   tool batch; image generation contributes either a success-shaped
   `ImageOperationCommitPart` or a terminal `ImageOperationTerminalCommitPart`;
17. once all sibling tool calls in the batch have completed, the agent builds one
    `ToolBatchTranscriptCommitBundle`;
18. one session transaction durably writes the `ToolResults` message for the
    whole batch, then the canonical generated assistant messages, assistant image
    handle records, replay sidecars, and audit rows after it;
19. the machine observes the transaction commit id and terminalizes the image
    operation as generated;
20. surfaces report the generated terminal outcome.

Generated success requires scoped-override restoration, committed image receipts,
and one atomic batch transcript commit containing the batch `ToolResults`
message, canonical generated assistant messages, assistant image handle records,
replay sidecars, and audit rows. That
transaction is keyed by `ToolBatchTranscriptCommitId(turn_id, tool_batch_id)` and
must be idempotent: retrying it either writes the whole set once or observes the
already-written set.

Normal model tool calls are the only path that can append generated assistant
images to the conversational transcript. Staged, realtime/direct, or external
dispatch requests that are not inside an active assistant tool-call/tool-result
batch use a separate machine ingress:

```rust
pub enum ImageGenerationIngress {
    AssistantToolBatch {
        turn_id: TurnId,
        tool_batch_id: ToolBatchId,
        tool_call_id: ToolCallId,
    },
    ExternalImageOperation {
        request_id: ExternalImageOperationRequestId,
        surface_id: SurfaceId,
    },
}

pub enum ImageOperationCommitProtocol {
    ToolBatchTranscript {
        commit_id: ToolBatchTranscriptCommitId,
    },
    ExternalArtifactOnly {
        commit_id: ExternalImageArtifactCommitId,
    },
}
```

`ExternalArtifactOnly` commits blobs, audit rows, retained image anchors,
neutral `GeneratedImageHandleRecord`s for
`GeneratedImageHandle::ExternalArtifact`, and replay sidecars, then returns an
image result to the calling surface. It never mints `AssistantImageId` or an
assistant-specific handle record, because no assistant transcript content exists.
Each external artifact handle resolves through a `RetainedImageAnchorRecord`
whose source is `RetainedImageAnchorSource::ExternalImageArtifact`; that anchor
is the canonical media and replay location. Replay attachments for this path are
keyed by `ProviderReplayOwnerRef::RetainedImageAnchor`. The path does not
allocate transcript message/block ids, does not append an assistant message, and
therefore has no tool-result adjacency to preserve. If a surface asks an external
operation to append conversational assistant content, the machine denies it with
a typed unsupported-ingress terminal; surfaces do not synthesize transcript
effects outside the tool-batch commit protocol.

Scoped override apply and restore are both machine transitions. If apply fails,
the provider executor is not called. If restore fails after provider execution,
the staged provider output receipt is retained as failure evidence, success is
unreachable, and the operation terminalizes through the typed restore-failure
path.

```rust
pub struct ToolBatchTranscriptCommitBundle {
    pub commit_id: ToolBatchTranscriptCommitId,
    pub turn_id: TurnId,
    pub tool_batch_id: ToolBatchId,
    pub append_allocation: TranscriptAppendAllocationReceipt,
    pub participant_parts: Vec<ToolBatchParticipantCommitPart>,
    pub tool_results_message: ToolResultsMessage,
    pub post_tool_assistant_messages: Vec<AssistantMessageAppend>,
    pub generated_image_handles: Vec<GeneratedImageHandleRecord>,
    pub replay_attachments: Vec<ProviderReplayAttachmentAppend>,
    pub bundle_digest: CommitBundleDigest,
}

pub enum ToolBatchParticipantCommitPart {
    ImageOperation(ImageOperationCommitPart),
    ImageOperationTerminal(ImageOperationTerminalCommitPart),
    ToolResult(ToolResultCommitPart),
}

pub struct ToolResultCommitPart {
    pub tool_call_id: ToolCallId,
    pub tool_result_receipt: ToolResultReceipt,
}

pub struct AssistantMessageAppend {
    pub allocation_id: TranscriptAppendAllocationId,
    pub message_id: MessageId,
    pub message: AssistantMessage,
    pub transcript_position: TranscriptAppendPosition,
}

pub struct ProviderReplayAttachmentAppend {
    pub allocation_id: TranscriptAppendAllocationId,
    pub owner: ProviderReplayOwnerRef,
    pub attachment: ProviderReplayAttachment,
    pub digest: ProviderReplayAttachmentDigest,
}

pub struct TranscriptAppendAllocationReceipt {
    pub allocation_id: TranscriptAppendAllocationId,
    pub commit_id: ToolBatchTranscriptCommitId,
    pub tool_results_message_id: MessageId,
    pub assistant_message_ids: Vec<MessageId>,
    pub block_ids: Vec<BlockId>,
    pub positions: Vec<TranscriptAppendPosition>,
    pub allocation_digest: TranscriptAppendAllocationDigest,
}

pub struct ImageOperationCommitPart {
    pub operation_id: ImageOperationId,
    pub provider_dispatch_envelope: ProviderDispatchEnvelopeReceipt,
    pub provider_call_attempt: ProviderCallAttemptReceipt,
    pub provider_output_receipt: ProviderOutputReceipt,
    pub image_receipts: Vec<CommittedImageReceipt>,
    pub audit_record: ImageGenerationAuditRecord,
    pub tool_result_receipt: ToolResultReceipt,
}

pub struct ImageOperationTerminalCommitPart {
    pub operation_id: ImageOperationId,
    pub terminal: ImageOperationTerminalClass,
    pub evidence: ImageOperationTerminalEvidence,
    pub tool_result_receipt: ToolResultReceipt,
}

pub enum ImageOperationTerminalEvidence {
    ProviderParamsDenied {
        receipt: ProviderParamsDenialReceipt,
    },
    ProviderPlanningDenied {
        receipt: ProviderPlanningDenialReceipt,
    },
    SourceResolutionDenied {
        decision: ImageSourceResolutionDecision,
    },
    ProjectionDenied {
        attempt: ProjectionAttemptRecord,
    },
    ProviderConsumptionDenied {
        attempt: ProjectionAttemptRecord,
        preflight_input: ProviderConsumptionPreflightInputRef,
        preflight_receipt: ProviderConsumptionPreflightReceiptRef,
    },
    ProviderConsumptionPreflightEvaluatorUnavailable {
        attempt: ProjectionAttemptRecord,
        preflight_input: ProviderConsumptionPreflightInputRef,
        provider_profile_version: ProviderProfileVersion,
    },
    ProviderConsumptionPlanMismatch {
        attempt: ProjectionAttemptRecord,
        preflight_input: ProviderConsumptionPreflightInputRef,
        preflight_receipt: ProviderConsumptionPreflightReceiptRef,
        approved_consumption_plan: ProviderConsumptionPlanRef,
    },
    ProjectionSnapshotMismatch {
        attempt: ProjectionAttemptRecord,
        evidence_digest: ProjectionFailureEvidenceDigest,
    },
    ProjectionSnapshotPersistFailed {
        attempt: ProjectionAttemptRecord,
        error_digest: ProjectionSnapshotStoreErrorDigest,
    },
    ReplayMaterialLost {
        decision_id: ReplayMaterialLostDecisionId,
        owner: ProviderReplayOwnerRef,
    },
    UnsupportedIngress {
        ingress: ImageGenerationIngress,
        reason: UnsupportedImageOperationIngressReason,
    },
    ScopedOverrideApplyFailed {
        decision_id: MachineDecisionId,
        error_digest: ScopedOverrideErrorDigest,
    },
    ScopedOverrideRestoreFailed {
        decision_id: MachineDecisionId,
        error_digest: ScopedOverrideErrorDigest,
    },
    ProviderCallAttemptReceiptWriteFailed {
        operation_id: ImageOperationId,
        error_digest: ReceiptWriteErrorDigest,
    },
    DispatchSerializationFailed {
        plan_digest: ProviderConsumptionPlanDigest,
        error_digest: ProviderDispatchSerializationErrorDigest,
    },
    BlobCommitFailed {
        operation_id: ImageOperationId,
        provider_output_receipt: ProviderOutputReceipt,
        error_digest: BlobCommitErrorDigest,
    },
    BatchCommitFailed {
        commit_id: ToolBatchTranscriptCommitId,
        error_digest: BatchCommitErrorDigest,
    },
    ExternalArtifactCommitFailed {
        commit_id: ExternalImageArtifactCommitId,
        error_digest: BatchCommitErrorDigest,
    },
    ProviderAttemptUnknown {
        provider_call_attempt: ProviderCallAttemptReceipt,
        retry_policy: ProviderRetryPolicySnapshot,
    },
    ProviderAttemptNoOutput {
        provider_call_attempt: ProviderCallAttemptReceipt,
        absence_receipt: ProviderNoOutputReceipt,
        retry_policy: ProviderRetryPolicySnapshot,
    },
    ProviderRetryExhausted {
        retry_policy: ProviderRetryPolicySnapshot,
        reason: ProviderRetryExhaustedReason,
    },
    UnknownProviderOutcome {
        provider_call_attempt: ProviderCallAttemptReceipt,
        retry_policy: ProviderRetryPolicySnapshot,
    },
    ProviderOutputWithoutImage {
        provider_call_attempt: ProviderCallAttemptReceipt,
        provider_output_receipt: ProviderOutputReceipt,
        classification: ProviderOutputClassificationReceipt,
    },
}

pub struct ProviderOutputClassificationReceipt {
    pub receipt_id: ProviderOutputClassificationReceiptId,
    pub provider_output_receipt_id: ProviderOutputReceiptId,
    pub attempt_id: ProviderCallAttemptId,
    pub correlation_id: ProviderCorrelationId,
    pub dispatch_envelope_digest: ProviderDispatchEnvelopeDigest,
    pub provider: ProviderId,
    pub profile_version: ProviderProfileVersion,
    pub schema: ProviderOutputClassificationSchema,
    pub schema_version: ProviderOutputClassificationSchemaVersion,
    pub reason: ProviderOutputWithoutImageReason,
    pub evidence_digest: ProviderOutputClassificationEvidenceDigest,
}

pub struct ProviderOutputClassificationEvidence {
    pub provider_output_receipt_id: ProviderOutputReceiptId,
    pub attempt_id: ProviderCallAttemptId,
    pub correlation_id: ProviderCorrelationId,
    pub dispatch_envelope_digest: ProviderDispatchEnvelopeDigest,
    pub provider: ProviderId,
    pub profile_version: ProviderProfileVersion,
    pub schema: ProviderOutputClassificationSchema,
    pub schema_version: ProviderOutputClassificationSchemaVersion,
    pub reason: ProviderOutputWithoutImageReason,
    pub evidence_digest: ProviderOutputClassificationEvidenceDigest,
}

pub enum ProviderOutputWithoutImageReason {
    Refusal,
    TextOnlyResponse,
    PolicyFiltered,
    ProviderReturnedNoUsableImage,
}

pub enum UnsupportedImageOperationIngressReason {
    ExternalOperationRequestedAssistantTranscriptAppend,
    ToolBatchOperationRequestedExternalArtifactOnly,
    UnsupportedSurfaceIngress,
}

pub enum ProviderRetryExhaustedReason {
    NoOutputRetryBudgetExhausted,
    UnknownOutcomeCannotBeRetried,
    RetryPolicyDeniedNewAttempt,
}

pub struct ProviderNoOutputReceipt {
    pub receipt_id: ProviderNoOutputReceiptId,
    pub attempt_id: ProviderCallAttemptId,
    pub provider: ProviderId,
    pub target_model: ModelId,
    pub provider_profile_version: ProviderProfileVersion,
    pub correlation_id: ProviderCorrelationId,
    pub proof_schema: ProviderNoOutputProofSchema,
    pub proof_schema_version: ProviderNoOutputProofSchemaVersion,
    pub proof_artifact: ProviderNoOutputProofArtifactRef,
    pub proof_digest: ProviderNoOutputProofDigest,
}

pub struct ProviderRetryPolicySnapshot {
    pub policy_id: ProviderRetryPolicyId,
    pub policy_version: ProviderRetryPolicyVersion,
    pub provider: ProviderId,
    pub target_model: ModelId,
    pub provider_profile_version: ProviderProfileVersion,
    pub operation_id: ImageOperationId,
    pub attempt_count: u32,
    pub retry_budget: u32,
    pub provider_resume_capability: ProviderResumeCapabilityEvidence,
    pub retry_decision_digest: ProviderRetryPolicyDigest,
}

pub enum ProviderRetryDecision {
    StartNewAttempt {
        retry_policy: ProviderRetryPolicySnapshot,
    },
    TerminalNoOutputRetryExhausted {
        retry_policy: ProviderRetryPolicySnapshot,
    },
    TerminalUnknownProviderOutcome {
        retry_policy: ProviderRetryPolicySnapshot,
    },
}

pub struct ProviderDispatchEnvelopeReceipt {
    pub receipt_id: ProviderDispatchEnvelopeReceiptId,
    pub provider: ProviderId,
    pub target_model: ModelId,
    pub consumption_plan_digest: ProviderConsumptionPlanDigest,
    pub dispatch_envelope_ref: ProviderDispatchEnvelopeRef,
    pub dispatch_envelope_digest: ProviderDispatchEnvelopeDigest,
}

pub struct ProviderCallAttemptReceipt {
    pub attempt_id: ProviderCallAttemptId,
    pub operation_id: ImageOperationId,
    pub provider: ProviderId,
    pub target_model: ModelId,
    pub request_digest: RequestDigest,
    pub consumption_plan_digest: ProviderConsumptionPlanDigest,
    pub dispatch_envelope_digest: ProviderDispatchEnvelopeDigest,
    pub idempotency_key: ProviderIdempotencyKey,
    pub correlation_id: ProviderCorrelationId,
}

pub struct ProviderOutputReceipt {
    pub receipt_id: ProviderOutputReceiptId,
    pub attempt_id: ProviderCallAttemptId,
    pub operation_id: ImageOperationId,
    pub provider: ProviderId,
    pub target_model: ModelId,
    pub request_digest: RequestDigest,
    pub consumption_plan_digest: ProviderConsumptionPlanDigest,
    pub dispatch_envelope_digest: ProviderDispatchEnvelopeDigest,
    pub idempotency_key: ProviderIdempotencyKey,
    pub correlation_id: ProviderCorrelationId,
    pub output_parts: Vec<ProviderOutputPartReceipt>,
    pub metadata_artifact: ProviderOutputMetadataArtifactRef,
    pub metadata_artifact_digest: ProviderOutputMetadataDigest,
    pub native_metadata_digest: NativeMetadataDigest,
}

pub struct ProviderOutputMetadataArtifact {
    pub artifact_ref: ProviderOutputMetadataArtifactRef,
    pub provider_text: ProviderTextDisposition,
    pub revised_prompt: RevisedPromptDisposition,
    pub provider_trace: Vec<ProviderTraceEvidence>,
    pub replay_attachments: Vec<ProviderReplayAttachment>,
    pub warnings: Vec<ImageGenerationWarning>,
    pub native_metadata: ProviderNativeMetadataEnvelope,
    pub artifact_digest: ProviderOutputMetadataDigest,
}

pub enum ProviderOutputPartReceipt {
    Image {
        temp_blob_id: TempBlobId,
        media_type: MediaType,
        byte_len: u64,
        byte_sha256: String,
    },
    Text {
        artifact_ref: TextArtifactRef,
        byte_sha256: String,
    },
}

pub struct ToolBatchCommitRecord {
    pub turn_id: TurnId,
    pub tool_batch_id: ToolBatchId,
    pub commit_id: ToolBatchTranscriptCommitId,
    pub participants: Vec<ToolBatchParticipantRef>,
    pub bundle_digest: CommitBundleDigest,
}

pub struct ExternalImageArtifactCommitRecord {
    pub request_id: ExternalImageOperationRequestId,
    pub commit_id: ExternalImageArtifactCommitId,
    pub operation_id: ImageOperationId,
    pub image_receipts: Vec<CommittedImageReceipt>,
    pub retained_image_anchors: Vec<RetainedImageAnchorRecord>,
    pub generated_image_handles: Vec<GeneratedImageHandleRecord>,
    pub replay_attachments: Vec<ProviderReplayAttachment>,
    pub audit_record: ImageGenerationAuditRecord,
    pub bundle_digest: CommitBundleDigest,
}
```

`ProviderDispatchEnvelopeReceipt` is durable after local serialization succeeds
and before any external attempt exists. A serialization failure is therefore a
typed local dispatch failure, not an unknown external outcome.
`ProviderCallAttemptReceipt` is durable before the external provider call starts.
It records the idempotency/correlation key and the exact request/plan/dispatch
digests. If recovery sees a provider call attempt without a provider output
receipt, it does not blindly reissue the call. It asks the provider profile
whether the attempt can be resumed or queried by idempotency/correlation key;
that answer is `ProviderResumeCapabilityEvidence`, not retry policy. The catalog
machine owns the retry policy and snapshots it as `ProviderRetryPolicySnapshot`
with policy id/version, provider, target model, provider profile version,
operation id, attempt count, retry budget, and provider resume evidence. If the
provider proves no output exists, the shell persists a `ProviderNoOutputReceipt`
with target model, provider profile version, proof schema, proof artifact ref,
and proof digest. The machine consumes that receipt plus the retry-policy
snapshot and either starts a new `ProviderCallAttemptReceipt` under the retry
budget or terminalizes with provider-no-output-retry-exhausted. If the provider
cannot prove no output exists and cannot recover output, the machine terminalizes
the operation through a typed unknown-provider-outcome class. Success remains
unreachable without a `ProviderOutputReceipt`.

Store/session owns the recovery-critical receipt APIs and mechanical persistence
retention for provider params denials, approved and denied provider planning,
projection history observations, provider consumption plans, dispatch envelopes,
provider call attempts, provider no-output proofs, provider outputs, provider
output classifications, tool results, transcript append allocations, and batch
commit records. Semantic retention is separate: the catalog machine owns
retention epochs, replay-material-lost decisions, anchor visibility, and whether
retained media remains selectable. The store may enforce TTL/GC and supply
observations, but it must not decide semantic source eligibility, replay
eligibility, or terminal classes. The shell may stage receipts only through those
APIs. Recovery reconstructs machine state from machine events/snapshots first;
only then does it load the receipt ids and digests referenced by machine state.
It never discovers operation truth by scanning store receipts and never
reconstructs receipt truth from local runtime memory.
The MeerkatMachine emits the external provider-call effect only after observing
the durable `ProviderCallAttemptReceipt`. If the receipt write fails, the machine
terminalizes or retries the provider-call-attempt-staging phase through a typed
receipt-write-failed path; the shell must not send the external request from a
local-only receipt.

`ProviderOutputReceipt` is a durable provider-output spool receipt. It owns the
post-provider, pre-final-blob state: temporary blob ids or artifact refs,
digests, media types, byte lengths, request/plan/dispatch digests, the exact
`ProviderCallAttemptId`, idempotency key, correlation id, and a durable metadata
artifact ref. A provider output without that call-attempt binding is stale
evidence and cannot be consumed by the machine. `ProviderOutputMetadataArtifact`
owns the provider text, revised
prompt, warnings, provider trace evidence, replay attachment payloads, and native
metadata needed to construct the final audit rows and replay sidecars. It is
pre-commit spool material, not the canonical future replay store. The atomic
transcript commit writes `ProviderReplayAttachment` sidecar rows as the sole
future replay source and audit rows with trace/digest evidence. The final blob
store commit consumes temporary bytes and produces `CommittedImageReceipt`s. A
crash after provider output receipt staging but before final blob commit is
therefore recoverable without calling the provider again; recovery validates temp
bytes and metadata artifact digests against the receipt and resumes blob commit.
If a provider output receipt contains no usable image parts, the provider profile
classifies native metadata into `ProviderOutputClassificationEvidence`. The shell
persists that evidence through store/session receipt APIs as a durable
`ProviderOutputClassificationReceipt`. The receipt carries profile/schema
versions and an evidence digest; the machine consumes the receipt and terminalizes
through `ProviderOutputWithoutImage`. The provider does not write durable
recovery state, the machine does not parse provider-native metadata, and the
shell does not invent the terminal reason.
That terminal still contributes a failure tool result through
`ImageOperationTerminalCommitPart`; it is neither generated success nor
provider-attempt absence.

The batch commit bundle is durable command payload, not lifecycle truth. The
catalog `MeerkatMachine` owns the tool-batch commit sub-protocol: batch commit
phase, participant membership, commit outcome, and recovery terminalization.
Individual participants contribute commit parts and then observe the machine's
batch result. Store/session owns transcript id and position allocation: before
the bundle is built, the commit sub-protocol obtains a durable
`TranscriptAppendAllocationReceipt`; the atomic append transaction consumes that
exact allocation and never generates fresh ids during retry. Replay attachment
sidecar storage participates in this same session transaction. The transaction
writes the whole `ToolResults` message for every sibling tool call in the batch,
canonical `AssistantMessageAppend` records after that message, replay attachment
sidecar rows keyed by the allocated `TranscriptBlockRef`s, and audit rows, or
writes nothing. If sidecar persistence fails, the whole append transaction fails
and the MeerkatMachine tool-batch commit sub-protocol retries or terminalizes the
batch commit failure; it must not commit transcript/audit rows without their
required sidecars. Message ids, block ids, assistant message metadata, stop
reasons, block order, replay attachments, and transcript position are therefore
part of the durable commit payload, not reconstructed by ad hoc append glue.
Non-image tool results and image-generation outputs share the same transcript
commit authority.

Recovery loads staged participant receipts first. Non-image tool calls use
`ToolResultReceipt`; image operations additionally use provider output and image
commit receipts. Failed image operations contribute
`ImageOperationTerminalCommitPart` with a failure `ToolResultReceipt`, so sibling
tool results never wait for a success-shaped image commit part. If an image
operation has a durable provider call attempt but no durable provider output
receipt, recovery follows the provider-resume/no-output/unknown-outcome policy
above; it does not retry blindly. A `ProviderNoOutputReceipt` is validated
against the attempt's target model, provider profile version, proof schema, proof
artifact, and digest before it feeds the machine-owned
`ProviderRetryPolicySnapshot`; that snapshot either starts a new attempt under
budget or terminalizes provider-no-output-retry-exhausted. If no provider call
attempt exists, recovery may start provider execution or terminalize through
provider-retry-exhausted according to the same machine-owned retry policy. If the
output receipt exists, recovery never calls the provider again for that
operation; it proceeds to restore/commit or terminalizes with typed
restore/commit failure. Non-image tool calls are likewise not re-executed once
their `ToolResultReceipt` exists. Once all participants are ready, the
MeerkatMachine tool-batch commit sub-protocol loads
`ToolBatchCommitRecord`, validates the bundle digest and participant receipts,
and retries the idempotent session transaction. If the bundle is missing or
mismatched, the batch remains uncommitted and the machine either retries the
same bundle from participant receipts/source truth or terminalizes the whole
batch commit with a typed recovery failure. Image operations project that batch
outcome; they do not individually decide transcript success. A machine terminal
state must never claim success while the effective model is unrestored or while
the durable session transcript lacks the corresponding atomic batch commit.

Tool-result adjacency remains mandatory. Generated image content is appended
after the tool result, never between a tool-use block and its result.

### Source images and typed history selectors

`source_images` and `reference_images` remain explicit handles. They are the
only way for the active model to say "use this image as an edit source or
generation reference".

Passive history projection and edit/reference source authority are different
facts. Projection policy may include visible prior assistant images as passive
context for providers that can consume image history, but it must not reinterpret
passive context as an edit source or generation reference. Edit/source semantics
require typed `source_images` or `reference_images` selectors and a
machine-owned source-resolution decision.

Passive history selection is deterministic and machine-owned:

- explicit `source_images` and `reference_images` resolve first;
- passive candidates are visible image-bearing transcript blocks before the
  request boundary that are not already selected as edit sources or generation
  references;
- candidates are ordered by transcript order, newest first;
- the machine snapshots the provider's passive-history channel and limits into
  `ProviderProjectionCapabilitySnapshot`, selects at most
  `min(policy.max_passive_history_images, capability.max_passive_history_images)`
  candidates, and enforces
  `min(policy.max_passive_history_bytes, capability.max_passive_history_bytes)`
  using `ImageCandidateBudgetObservation`;
- a non-`None` passive-history capability is usable only when its
  `PassiveHistorySemanticsProof` guarantees context-only semantics and explicitly
  rules out edit-source and generation-reference authority;
- omitted candidates are recorded in the `ImageProjectionDecision` as
  `AlreadySelectedAsSourceOrReference`, `PassiveHistoryLimit`,
  `PassiveHistoryByteBudget`, `OutsideWindow`, or `Compacted`;
- if the capability snapshot declares no passive image-history channel, the
  machine selects zero passive images and records
  `ProviderHasNoPassiveHistoryChannel` omissions instead of letting provider
  lowering decide.

The target architecture supports visible-history references through typed tool
arguments, not prompt-string inference:

```rust
pub enum ImageSourceRef {
    AssistantImage { image_id: AssistantImageId },
    TranscriptBlock { message_id: MessageId, block_id: BlockId },
    RetainedImageAnchor { anchor_id: RetainedImageAnchorId },
    LatestVisibleAssistantImage {
        boundary: LatestVisibleImageBoundary,
    },
}

pub enum ConcreteImageSourceRef {
    AssistantImage { image_id: AssistantImageId },
    TranscriptBlock { message_id: MessageId, block_id: BlockId },
    RetainedImageAnchor { anchor_id: RetainedImageAnchorId },
}

pub enum ImageCandidateOrder {
    Transcript(TranscriptOrderKey),
    ExternalArtifact {
        commit_id: ExternalImageArtifactCommitId,
        output_index: u32,
    },
}

pub enum ImageCandidateSourceOrigin {
    UserMessage,
    AssistantMessage,
    ToolResult,
    RetainedAnchor,
    ExternalArtifact,
}

pub enum LatestVisibleImageBoundary {
    CurrentTranscriptBoundary,
    BeforeMessage { message_id: MessageId },
}

pub struct RetainedImageAnchorRecord {
    pub anchor_id: RetainedImageAnchorId,
    pub source: RetainedImageAnchorSource,
    pub created_by_decision: MachineDecisionId,
    pub retention_epoch: RetentionEpoch,
    pub visibility: RetainedImageAnchorVisibility,
    pub media: ImageCandidateMediaObservation,
    pub replay: ProviderReplayRef,
}
```

Surface-originated image handles use a separate machine command input. They are
opaque domain handles issued by Meerkat session/store code to surfaces, not raw
blob ids and not model-facing `ImageSourceRef`s:

```rust
pub enum ImageSourceResolutionInput {
    ModelFacing(ImageSourceRef),
    SurfaceImageHandle(SurfaceImageHandleInput),
}

pub struct SurfaceImageHandleInput {
    pub handle: SurfaceImageHandle,
    pub claimed_session_id: SessionId,
    pub caller: SurfaceImageHandleCaller,
    pub declared_media_type: MediaType,
}

pub struct SurfaceImageHandleCaller {
    pub surface_id: SurfaceId,
    pub principal: SurfacePrincipalRef,
}
```

The source-resolution transition converts `SurfaceImageHandle` into a domain
source, such as `TranscriptBlock`, `AssistantImage`, or `RetainedImageAnchor`,
only after checking session ownership, retention, visibility, and reachability.
If the handle is not a valid image source for that session, the transition
returns a typed denial. Surfaces may pass surface image handles into this
command, but they must not authorize or reinterpret them themselves.
`declared_media_type` is a caller claim used only for diagnostics and mismatch
reporting; media legality uses store-observed media facts from
`SurfaceImageHandleObservation`. Handle/session ownership and visibility are
explicit observation fields; they must not be inferred from blob-store records.
Surface handles are not bearer-only capabilities: the machine compares the
caller surface id and principal in `SurfaceImageHandleInput` with the handle
owner observation before it authorizes the resolved source.

`RetainedImageAnchor` is also machine-owned. Compaction may propose anchors, and
external artifact commits may propose anchors for images with no transcript
content, but the catalog machine decides anchor creation and visibility, records
`RetainedImageAnchorRecord`, and advances the retention epoch. The store persists
anchor records and supplies observations; source resolution decides whether an
anchor is selectable for a request. Hidden compacted media is not selectable
unless such a machine-created anchor exists.

The active model may read the user's text and choose
`LatestVisibleAssistantImage`, but Meerkat resolves only the typed selector
through a machine-owned source-resolution transition.
Runtime code must not inspect prompt strings for phrases like "previous image",
"existing image", "latest image", or "that image".

`MessageId` and `BlockId` are transcript-owned domain handles. Store/session
allocates them durably through `TranscriptAppendAllocationReceipt` before the
atomic append transaction, then consumes that allocation when writing the
canonical transcript row. The allocated ids are the only public handles for
transcript-block selection. A `TranscriptBlockRef` is valid only when it resolves
through that persisted message/block identity in the target session; vector
indexes, display order offsets, and provider-local ids are not accepted as
substitutes.

The source-resolution handoff is explicit. The shell does not return the chosen
image and does not pre-filter semantic eligibility. It returns a complete,
mechanically derived candidate observation set at a transcript boundary. The set
includes all image-bearing transcript blocks and explicit image handles that are
mechanically reachable in the requested session scope, not only candidates the
shell believes are visible or usable:

```rust
pub struct ImageSourceResolutionObservation {
    pub operation_id: ImageOperationId,
    pub transcript_boundary: TranscriptBoundary,
    pub requested: Vec<ImageSourceResolutionInput>,
    pub candidate_observations: Vec<ImageSourceCandidateObservation>,
    pub surface_handle_observations: Vec<SurfaceImageHandleObservation>,
    pub proof: SourceResolutionObservationValidity,
    pub visibility_epoch: VisibilityEpoch,
    pub retention_epoch: RetentionEpoch,
    pub observation_digest: SourceResolutionObservationDigest,
}

pub struct ImageSourceCandidateObservation {
    pub candidate_ref: ConcreteImageSourceRef,
    pub order: ImageCandidateOrder,
    pub media: ImageCandidateMediaObservation,
    pub structural_facts: ImageCandidateStructuralFacts,
    pub visibility_facts: ImageVisibilityFacts,
    pub retention_facts: ImageRetentionFacts,
}

pub enum ImageCandidateMediaObservation {
    BlobReferenced {
        blob_id: BlobId,
        media_type: MediaType,
        budget: ImageCandidateBudgetObservation,
        origin: ImageCandidateSourceOrigin,
    },
    BlobRecordMissing {
        blob_id: BlobId,
        origin: ImageCandidateSourceOrigin,
        evidence_digest: ImageCandidateMediaEvidenceDigest,
    },
    BlobUnreachable {
        blob_id: BlobId,
        origin: ImageCandidateSourceOrigin,
        evidence_digest: ImageCandidateMediaEvidenceDigest,
    },
    MediaFactsUnavailable {
        origin: ImageCandidateSourceOrigin,
        reason: ImageCandidateMediaUnavailableReason,
        evidence_digest: ImageCandidateMediaEvidenceDigest,
    },
}

pub enum ImageCandidateMediaUnavailableReason {
    MissingMediaType,
    MissingByteLength,
    MissingDigest,
    CorruptStoreRecord,
    UnsupportedCanonicalMediaShape,
}

pub struct ImageCandidateBudgetObservation {
    pub byte_len: u64,
    pub byte_sha256: String,
    pub budget_digest: ImageCandidateBudgetDigest,
}

pub enum SurfaceImageHandleObservation {
    Resolved {
        input: SurfaceImageHandleInput,
        resolved_source: ConcreteImageSourceRef,
        resolved_handle: GeneratedImageHandle,
        resolved_blob_id: BlobId,
        handle_owner: SurfaceImageHandleOwnerObservation,
        store_record: BlobStoreRecordObservation,
        reachability: BlobReachabilityObservation,
        visibility_facts: ImageVisibilityFacts,
        retention_facts: ImageRetentionFacts,
        observation_digest: SurfaceImageHandleObservationDigest,
    },
    Unresolved {
        input: SurfaceImageHandleInput,
        reason: SurfaceImageHandleObservationFailure,
        evidence_digest: SurfaceImageHandleObservationDigest,
    },
}

pub enum SurfaceImageHandleObservationFailure {
    HandleNotFound,
    ForeignSession { observed_session_id: SessionId },
    CallerMismatch {
        issued_to_surface: SurfaceId,
        issued_to_principal: SurfacePrincipalRef,
        caller: SurfaceImageHandleCaller,
    },
    OwnerUnknown,
    OwnerRecordCorrupt { evidence_digest: SurfaceHandleOwnerCorruptionDigest },
    OwnerRedacted { redaction_policy: SurfaceHandleOwnerRedactionPolicy },
    ExpiredHandle { handle_epoch: SurfaceHandleEpoch },
    BlobRecordMissing { blob_id: BlobId },
    BlobUnreachable { blob_id: BlobId },
    VisibilityRecordMissing,
    RetentionRecordMissing,
    MediaMismatch {
        declared_media_type: MediaType,
        observed_media_type: MediaType,
    },
}

pub struct SurfaceImageHandleOwnerObservation {
    pub handle: SurfaceImageHandle,
    pub owning_session_id: SessionId,
    pub issued_to_surface: SurfaceId,
    pub issued_to_principal: SurfacePrincipalRef,
    pub handle_epoch: SurfaceHandleEpoch,
    pub owner_digest: SurfaceImageHandleOwnerDigest,
}

pub struct SourceResolutionObservationProof {
    pub store_snapshot: StoreSnapshotRef,
    pub transcript_enumeration: TranscriptEnumerationProof,
    pub blob_index_enumeration: BlobIndexEnumerationProof,
    pub visibility_enumeration: VisibilityEnumerationProof,
    pub retention_enumeration: RetentionEnumerationProof,
    pub surface_handle_owner_enumeration: SurfaceHandleOwnerEnumerationProof,
    pub verifier: SourceObservationVerifier,
    pub proof_digest: SourceObservationProofDigest,
}

pub enum SourceObservationVerifier {
    StoreTranscriptIndex,
    StoreBlobReachabilityIndex,
    StoreVisibilityIndex,
    StoreRetentionIndex,
}

pub enum SourceResolutionObservationValidity {
    Verified { proof: SourceResolutionObservationProof },
    Unverifiable { reason: SourceObservationUnverifiableReason },
}

pub enum ImageSourceResolutionDecision {
    Resolved {
        decision_id: MachineDecisionId,
        decision_event_id: MachineEventId,
        resolved: Vec<ResolvedImageSource>,
        observation_digest: SourceResolutionObservationDigest,
        decision_digest: SourceResolutionDecisionDigest,
    },
    Denied {
        decision_id: MachineDecisionId,
        decision_event_id: MachineEventId,
        reason: ImageSourceResolutionDenial,
        observation_digest: SourceResolutionObservationDigest,
        decision_digest: SourceResolutionDecisionDigest,
    },
}
```

The catalog transition applies the selector semantics to those observations. It
chooses the latest visible assistant image, rejects mixed selector/handle sets,
applies visibility and retention policy, and emits a decision id. For
`SurfaceImageHandle` inputs, the shell supplies canonical store observations such
as handle/session ownership, resolved domain source, resolved generated-image
handle, resolved blob id, blob store record, reachability, visibility, and
retention facts. The machine authorizes the `resolved_source`, not the raw
`BlobId`; blob identity remains media evidence for that source. Negative facts
are observations too: missing
handles, known-foreign sessions, unknown/corrupt/redacted owners, expired
handles, missing blob records, unreachable blobs, absent visibility/retention
rows, and media mismatches are reported as
`SurfaceImageHandleObservation::Unresolved`. The machine decides whether those
observations authorize conversion to a domain source or require a typed denial.
The shell's facts can say what was observed, but the observation input carries
`SourceResolutionObservationValidity`.
Completeness is accepted only when `Verified` carries proofs against canonical
store transcript, blob reachability, visibility, and retention indexes. If the
proof is `Unverifiable` or mismatched, the machine denies source resolution with
a typed unverifiable-source-observation reason rather than selecting from a
partial set.
Projection can materialize only `ResolvedImageSource`s named by that decision.
If the transcript boundary, visibility epoch, retention epoch, proof digest, or
observation digest changes before projection binding, the source-resolution
decision is stale and must be rerun or terminalized through a typed
stale-source-resolution denial.

`ImageCandidateMediaObservation` records declared transcript/storage references
plus store-observed byte budget facts or typed negative media facts. It does not
require provider-ready hydration, but successful media observations are always
blob-backed because inline ingress media is externalized before canonical
transcript append. Missing blob records, unreachable blobs, and unverifiable
media facts are still candidate observations, not omissions. The machine decides
whether they cause source-resolution denial, passive-history omission, or later
projection failure. For successful media observations, byte length and digest
facts let the machine own passive-history byte-budget selection. Once the machine
resolves a source, projection hydrates that selected media into
`SelectedImageMedia` and validates the observed byte facts, or terminalizes
through the machine's missing/unsupported selected-media path.

Raw `BlobId` is not a source reference. Blob identity is storage truth for bytes,
not control truth for "which image should this operation use." If a surface
starts with an uploaded or displayed image, it must pass a Meerkat
`SurfaceImageHandle` to the machine-owned source-resolution command. The shell
may resolve that handle to store/blob observations, but the catalog transition
performs the access, visibility, retention, and session-scope decision and
returns either resolved domain image sources or a typed denial. Projection source
truth binds the machine decision id.
Surfaces pass user intent and display results; they do not perform semantic
image-source authorization themselves.

The policy is:

- requests may use concrete handles or typed selectors, but not both in the same
  `source_images` or `reference_images` set;
- `source_images` are edit inputs and `reference_images` are generation
  references;
- selected prior assistant images are materialized from machine-owned
  source-resolution decisions;
- unselected prior images are not sent;
- compaction or truncation decisions are visible in audit/projection metadata;
- provider adapters receive the selected projection, not raw session history.

If a typed latest-visible selector is supplied, the machine-owned
source-resolution transition selects the most recent eligible visible assistant
image by transcript order and records that decision. If no eligible image
exists, the machine terminalizes the operation with a typed source-resolution
denial. If selectors and concrete handles are mixed in the same image set,
validation fails typed. Silent text-only fallback is forbidden.

This turns a prompt such as "add the word Meerkat to the existing sign" into a
testable system property:

- the assistant must call `generate_image` with a typed source image selector;
- if that selector resolves, the provider receives actual image bytes;
- if it does not resolve, the request fails through machine-owned terminal
  semantics.

## Target Invariants

### Transcript invariants

- There is exactly one assistant message type.
- User and assistant visible media share `ContentBlock`.
- Generated images are assistant content, not tool logs.
- Tool calls remain ordered assistant items.
- Tool results remain separate messages so provider adjacency can be preserved.
- Reasoning/thinking remains assistant-only replay structure.
- Provider replay metadata is typed.

### Runtime invariants

- Image operation lifecycle is machine-owned.
- Scoped model override lifecycle is machine-owned.
- Provider calls and blob writes are shell mechanics.
- Provider outputs are evidence; machine terminal classes are truth.
- Provider-specific image model facts live in provider profiles.
- Effective model projections are derived from machine state.
- Projection selected/omitted manifests, snapshot binding, and
  projection-triggered terminal classes are machine-owned.
- Runtime code routes typed image selectors through the machine-owned
  source-resolution transition; it never derives selector semantics from prompt
  strings.

### Projection invariants

- Transcript projection is rebuildable.
- Projection selection policy is not hidden in provider adapters.
- Blob-backed media is hydrated before provider lowering.
- Missing selected blobs become machine-owned projection terminal facts.
- Shared projection emits canonical Meerkat content only.
- Provider role/schema quirks are handled in provider crates and recorded as
  provider-local lowering evidence.

### Surface invariants

- CLI, RPC, REST, MCP, Python, TypeScript, and WASM expose the same transcript
  facts.
- SDKs may offer convenience views for generated images, but those views derive
  from assistant content blocks.
- `blob/get` remains the byte retrieval path for durable image blobs.
- Surfaces do not infer provider execution semantics.

## Implementation Plan

The implementation plan is intentionally slice-based, not layer-based. Every
slice must leave the workspace buildable. If a semantic change removes a core
type, the same slice must update all provider, builtin, wire, SDK, and docs
callers needed for the gate to pass. No slice may rely on a later slice to repair
compilation.

### Slice 1: Canonical Transcript Cutover

- Rename `UserMessage.content` to `UserMessage.blocks` and add
  `UserMessageMeta`.
- Replace flat `AssistantMessage` with ordered assistant blocks and
  `AssistantMessageMeta`.
- Remove `BlockAssistantMessage`, `Message::BlockAssistant`,
  `AssistantBlock::Text`, and `AssistantBlock::Image`.
- Add `AssistantBlock::Content(ContentBlock::...)`,
  `AssistantContentOrigin::GeneratedImage`, opaque provider replay envelope
  refs, and neutral generated-image handle record types as inert schema. Do not
  emit new generated-image handles, generated-image assistant content, or replay
  sidecars until the catalog commit protocol owns `created_by_decision` and
  transcript allocation in Slice 6.
- In the same slice, update text extraction, tool-call iteration, display,
  indexing, compaction, hook rewrite helpers, provider serializer matches,
  wire decode/encode, REST/RPC/MCP surface history serialization, CLI/history
  rendering, Python SDK parsing, TypeScript SDK parsing, generated SDK types,
  and `artifacts/schemas/wire-types.json`.
- Add clean-cut serde for the whole transcript schema: accept canonical
  `role: "assistant"` ordered blocks and reject old flat assistant rows,
  `role: "block_assistant"` rows, and old `UserMessage.content` aliases.

Gate: `make check`, focused core/wire serde tests, provider-builder compile
tests, schema freshness, REST/RPC/MCP history tests, CLI history tests, SDK
parser/generated-type tests for canonical assistant rows, and negative tests
proving old assistant rows and aliases are rejected at public decode and store
load boundaries. Generated-image handle resolvers must reject records without a
machine decision id; Slice 1 may define the type but must not make it live.

### Slice 2: Structural Content Traversal

- Rewrite hydration, externalization, blob reachability, compaction, indexing,
  hooks, deferred prompts, deferred tool results, and SDK convenience views to
  walk `ContentBlock` structurally instead of by message role.
- Enforce blob-backed canonical media by externalizing inline ingress bytes at
  append boundaries and recording `ImageExternalizationReceipt`.
- Keep provider replay selection disabled in this slice except for compiling
  against the new structural traversal APIs. No selected-history replay policy,
  provider-consumption denial, or missing-selected-media terminal semantics may
  be introduced before the catalog machine owns the decisions in Slice 3 and the
  runtime materializes them in Slice 4.

Gate: focused tests proving user, assistant, tool-result, deferred prompt, and
deferred tool-result images externalize, hydrate, and collect blob ids through
role-neutral structural walkers. Tests in this slice must not assert semantic
projection selection behavior.

### Slice 3: Catalog Machine Authority

- Add catalog-owned image operation phases/effects for accepted/denied provider
  planning, source-resolution decisions, projection history observations,
  projection decisions, projection attempts, provider-consumption preflight,
  provider dispatch/call/output/classification receipt observations, blob commit
  receipt observations, tool-batch commit, external artifact-only commit, and
  terminal outcomes.
- Add catalog-owned retained-anchor visibility decisions and
  `GeneratedImageHandleRecord` resolution checks, with `AssistantImageId` limited
  to assistant transcript-visible outputs and `ExternalImageArtifactHandle`
  reserved for external artifact-only results.
- Add terminal classes for source-resolution denial, projection failure,
  provider-consumption denial across the full canonical manifest, replay material
  lost, unverifiable projection observation, dispatch serialization failure,
  provider-consumption preflight evaluator unavailable,
  provider-consumption plan mismatch, projection snapshot mismatch,
  unsupported ingress, provider attempt unknown/no-output, provider retry
  exhausted, unknown provider outcome, output-without-image, blob commit failure,
  batch commit failure, and scoped override apply/restore failure.
- Add catalog-owned `ProviderRetryPolicySnapshot` generation and observations for
  provider no-output / unknown-outcome recovery. Provider profiles may supply
  resume/query evidence, but they do not own retry budgets or terminal decisions.
- Generate production artifacts from the catalog. Production-only runtime machine
  state is forbidden.

Gate: machine codegen, drift check, runtime schema parity, runtime alphabet
parity, TLC verification, and focused runtime tests proving external provider
effects are emitted only after durable call-attempt receipt observation.

### Slice 4: Projection And Preflight Runtime

- Replace `projected_messages: Vec<Message>` with `ImageProjectionSnapshot` and
  materialize only machine-owned `ImageProjectionDecision`s.
- Implement `ProjectionHistoryObservation`, typed source-resolution observations,
  deterministic selected/omitted manifests, token-accounting proofs,
  tool-adjacency groups, replay refs, and stale/mismatch detection.
- Implement durable accepted provider planning receipts, preflight inputs,
  preflight receipts, provider-consumption plan receipts, and recovery rules that
  use the original provider/profile/plan identity or terminalize typed.
- Implement provider-consumption denial for text, images, reasoning, tool use,
  tool results, replay requirements, and tool adjacency before provider lowering.

Gate: projection/preflight tests for explicit sources, latest-visible selectors,
passive history, omitted items, missing blobs, oversized media, compaction
boundaries, provider-consumption denials, deterministic rebuild, stale source
resolution, projection observation denial, source observation denial, projection
snapshot mismatch, provider-consumption plan mismatch, missing selected media,
and preflight recovery.

### Slice 5: Provider Lowering And Replay

- Collapse provider handling to one `Message::Assistant` branch.
- Lower selected canonical content through OpenAI Responses, OpenAI Images API,
  Gemini, Anthropic, OpenAI-compatible, and realtime/text paths where applicable.
- Decode and preserve provider replay envelopes through provider-owned codecs.
- Record role/schema adaptation in provider-local lowering evidence, not in the
  shared projection snapshot.
- Bind provider output receipts and classification receipts to the exact
  `ProviderCallAttemptId`, dispatch digest, idempotency key, and correlation id.

Gate: provider request-builder tests proving approved selected user, assistant,
tool-result, reasoning, tool-use, and image-only content serializes legally; no
selected content is silently dropped or serialized as an empty assistant/model
message. Semantic denials belong to Slice 4 provider-profile preflight. Slice 5
builders may report only invariant violations or mechanical serialization
failures after receiving an approved manifest.

### Slice 6: Image Generation Commit Paths

- Wire privileged `generate_image` through the machine-owned tool-batch
  transcript commit protocol.
- Commit generated image bytes, generated image handle records, replay sidecars,
  audit rows, tool results, and assistant content messages in one atomic
  transaction after sibling tool calls finish.
- Make `AssistantContentOrigin::GeneratedImage`, `GeneratedImageHandleRecord`,
  and generated-image replay sidecars live only through this commit protocol.
  Every emitted handle record must carry the machine `created_by_decision`,
  transcript allocation, and commit digest that produced it.
- Wire `ExternalImageOperation` / `ExternalArtifactOnly` so it returns neutral
  external artifact handles and retained anchors, never assistant transcript
  content.
- Preserve tool-call/tool-result adjacency for normal, parallel, staged,
  realtime, and direct dispatch paths.

Gate: adjacency tests for assistant tool batches and parallel tool calls;
external artifact-only tests proving no assistant transcript append; recovery
tests for transcript allocation retry, replay sidecar failure, provider output
staging, blob commit, and batch commit.

### Slice 7: Docs, Examples, And Skills

- Update README, docs, guide examples, and `.claude/skills/meerkat-*` to explain
  canonical assistant content, neutral generated-image handles, typed image
  source selectors, and provider-specific parameter envelopes.
- Remove every `role: "block_assistant"` example and every compatibility alias
  from documentation and skills. Public wire/schema/SDK truth was already cut
  over in Slice 1; this slice cannot introduce or preserve alternate transcript
  contracts.

Gate: docs link/reference checks, docs snippet checks where available, and skill
examples that use the new model-facing tool shape without legacy transcript
roles.

### Slice 8: End-to-End Live Smokes

- OpenAI text model generating an OpenAI image.
- OpenAI text model generating a Gemini image.
- Gemini text model generating a Gemini image.
- Python SDK generating a Gemini image with provider-specific parameters.
- TypeScript SDK generating an OpenAI image with provider-specific parameters.
- Cross-provider history replay with artifact tracing:
  1. start with OpenAI text model;
  2. generate a Gemini image containing a nonce label such as `MKT76-{nonce}`;
  3. switch to Gemini text model;
  4. ask Gemini image generation to edit the typed latest visible assistant image
     and change only that label to `MKT76-{nonce}-EDIT`;
  5. switch back to OpenAI text model;
  6. ask it to read the latest image;
  7. assert image replay using blob SHA-256 evidence through first image ->
     Gemini edit request -> edited image -> OpenAI readback request.

Gate: live OpenAI/Gemini lanes, SDK live lanes, and scenario 76 trace. The final
text readback is useful but not primary proof; the blob-hash propagation trace is
the architectural proof.

## Rejected Designs

### Keep `AssistantBlock::Image`

Rejected because it creates a second image representation. User images,
tool-result images, and assistant images must share `ContentBlock::Image`.

### Let provider adapters scan raw history

Rejected because provider adapters would become hidden projection-policy owners.
They should lower selected content, not choose selected content.

### Treat image generation as an ordinary external tool

Rejected because the operation mutates session transcript state, commits blobs,
participates in scoped model routing, and has typed machine lifecycle. Ordinary
tools do not own those semantics.

### Model `gpt-image-*` as a normal chat session model

Rejected because OpenAI image models are image-generation backends in this
substrate, not long-lived Meerkat chat-session identities.

### Require explicit `source_images` for all edits forever

Rejected as a universal rule. Explicit sources are valuable, but visible
assistant image content is still transcript content for passive history context.
Machine-owned projection policy may select visible images as passive context
under deterministic limits, but only typed `source_images` or `reference_images`
can grant edit/reference authority. A provider backend that cannot represent
passive context separately gets zero passive selections or must reject that
purpose rather than lower it as a source/reference.

## Review Checklist

- Does the change preserve one assistant transcript model?
- Does visible media flow through `ContentBlock`?
- Does generated image provenance avoid creating a second content type?
- Does persistence reject old assistant rows unless an explicit offline cutover
  migration has converted them?
- Does hydration recurse through assistant content?
- Does projection produce an auditable `ImageProjectionSnapshot` when image
  replay is selected?
- Do provider adapters lower selected content without inventing selection?
- Do provider adapters avoid empty assistant/model messages for image-only
  assistant content?
- Are provider-specific parameters owned by provider crates?
- Are operation and scoped-override phases machine-owned?
- Are tool-call/tool-result adjacency constraints preserved?
- Do wire contracts and SDKs expose the same transcript truth?
- Do live tests prove image replay with real provider calls?

## References

- `docs/architecture/meerkat-runtime-dogma.md`
- `meerkat-core/src/types.rs`
- `meerkat-core/src/image_content.rs`
- `meerkat-tools/src/builtin/image_generation.rs`
- `meerkat-openai/src/client.rs`
- `meerkat-gemini/src/client.rs`
- OpenAI image generation guide:
  `https://developers.openai.com/api/docs/guides/image-generation`
- OpenAI Responses image generation tool:
  `https://developers.openai.com/api/docs/guides/tools-image-generation`
- Gemini image generation guide:
  `https://ai.google.dev/gemini-api/docs/image-generation`
- Gemini thought signatures:
  `https://ai.google.dev/gemini-api/docs/gemini-3#thought-signatures`
- Anthropic vision guide:
  `https://docs.anthropic.com/en/docs/build-with-claude/vision`
