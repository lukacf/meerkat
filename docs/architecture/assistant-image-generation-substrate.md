# ADR: Assistant Image Generation and Scoped Turn Switching

**Status**: Proposed
**Date**: 2026-04-26

## Problem

Issue [#360](https://github.com/lukacf/meerkat/issues/360) proposes assistant
image generation as a first-class Meerkat capability. The difficult part is not
that providers can generate images. The difficult part is that the clean
provider paths are asymmetric:

- OpenAI image generation is naturally a provider-native image tool/API behind a
  normal text-capable session model. A `gpt-5.5` session can use OpenAI image
  generation without changing the session LLM identity.
- Gemini image generation is naturally a scoped turn on a native image-output
  Gemini model. For a Gemini text session moving to a Gemini image model, the
  clean provider path is a temporary live model change that can preserve
  provider-native context where the target supports it.

The UX requirement is still one simple operation: the user asks for an image and
may choose a target such as default OpenAI image generation, Gemini Flash Image,
Gemini Pro Image, or a future provider target. The active chat model should not
have to learn provider-specific rituals.

For example:

- In a `gpt-5.5` session, the user may ask for the default image model.
- In the same `gpt-5.5` session, the user may later ask for a Gemini image model
  such as "Nano Banana".
- In a Gemini text session, the user may ask for a Gemini image model and expect
  the runtime to preserve as much Gemini-native context as possible.

Treating image generation as an ordinary external tool would put too much
semantic authority in the tool dispatcher. Treating it as just a public
`switch_model` call would leak runtime choreography into the model-facing UX.
Treating OpenAI and Gemini as if they had the same execution shape would make the
architecture pleasant on paper and brittle in production.

The same discussion also exposes a broader runtime feature: a model or user may
want to request that a future LLM call run on a different model for a bounded
scope. This ADR names that feature `switch_turn`. The final public name may be
`switch_model`, `switch_turn`, or a surface-specific alias, but the architecture
needs one shared scoped model override substrate for both explicit model-routing
control and implicit image-generation execution plans.

## Decision

### 1. `generate_image` is the model-facing primitive

Meerkat will expose a Meerkat-owned builtin tool named `generate_image`.
Provider-native image APIs are execution backends, not the public or
model-facing contract.

The active session model calls `generate_image` when it wants an image,
regardless of which provider will produce it. The request is a typed intent, not
a provider prompt string:

```rust
struct GenerateImageRequest {
    intent: ImageGenerationIntent,
    target: ImageGenerationTargetPreference,
    size: ImageSizePreference,
    quality: ImageQualityPreference,
    format: ImageFormatPreference,
    count: NonZeroU32,
}

enum ImageGenerationIntent {
    Generate {
        prompt: PromptText,
        prompt_source: PromptSource,
        reference_images: Vec<ImageSourceRef>,
    },
    Edit {
        instruction: PromptText,
        instruction_source: PromptSource,
        source_images: NonEmptyVec<ImageSourceRef>,
    },
}

struct PromptText {
    content: String,
}

enum PromptSource {
    UserProvided { message_id: MessageId },
    ModelDistilled { tool_call_id: ToolCallId },
    Hybrid {
        user_message_ids: Vec<MessageId>,
        tool_call_id: ToolCallId,
    },
}
```

`PromptText` is a validated UTF-8 text newtype with a model/profile-defined
maximum length and redaction policy. The single `tool_call_id` in
`PromptSource::Hybrid` is intentional: one model invocation may distill multiple
user messages into one typed image intent.

Image sources are typed references that can carry both durable Meerkat content
and provider-native continuity handles:

```rust
enum ImageSourceRef {
    Blob { blob_ref: BlobRef },
    TranscriptBlock { message_id: MessageId, block_id: BlockId },
    AssistantImage { image_id: AssistantImageId },
    ProviderNative {
        provider: ProviderId,
        handle: ProviderImageHandle,
        continuity: ImageContinuityDisposition,
        fallback_blob_ref: BlobRef,
    },
}

enum ImageContinuityDisposition {
    NotProvided,
    UnsupportedBySourceProvider,
    Available(ImageContinuityRef),
}
```

Provider-native handles are never the only durable source of truth. They may be
used when the selected target can consume them, especially for same-provider
Gemini image continuity. Cross-provider projection degrades to the fallback blob
or transcript image content.

The canonical handle for a generated image is `AssistantImageId`. `BlobRef` is
the storage projection for bytes. Operation id plus image index is provenance,
not identity. `AssistantImageRef` is an app-facing wrapper around
`AssistantImageId` plus display metadata.

```rust
struct AssistantImageRef {
    image_id: AssistantImageId,
    blob_ref: BlobRef,
    media_type: MediaType,
    width: u32,
    height: u32,
}
```

Exact field names may change, but the semantic contract is fixed:

- the active model expresses the image request once, as typed Meerkat intent
- target selection is typed and policy-visible
- edit requests carry non-empty source images by construction
- source image references can preserve provider-native continuity without
  making provider handles canonical transcript truth
- count is non-zero and validated against the selected target's maximum by the
  machine; Meerkat rejects unsupported counts instead of silently clamping
- provider-specific prompts, request bodies, tool choices, and model switches are
  derived execution details

### 2. Session model and image target are separate choices

The long-lived session LLM identity is distinct from the per-request image
generation target.

```rust
enum ImageGenerationTargetPreference {
    Auto,
    ProviderDefault { provider: ProviderId },
    Model { provider: ProviderId, model: ModelId },
}

struct ImageGenerationTargetCapabilities {
    hosted_image_generation_tool: bool,
    native_image_output: bool,
    custom_tools: bool,
    image_search_grounding: bool,
    image_continuity_tokens: ImageContinuityTokenSupport,
}
```

`Auto` and `ProviderDefault` are resolved by runtime policy, model capability
metadata, user/session defaults, and cost/safety constraints. `Model` means use
that concrete image-capable target or fail with a typed unsupported-target
terminal. User confirmation before spending money or crossing providers is
policy, not a model-facing target preference. If that policy requires suspension
and the dispatching surface can support it, the image operation enters
`AwaitingApproval(approval_id)` and records a `ModelRoutingApprovalRequest`. If
the surface cannot support suspension, the machine terminalizes the image
operation with a typed denial; surfaces do not synthesize confirmation outcomes.

The selected provider, backend, model target, maximum count, and capability facts
for each operation are recorded as part of the resolved execution plan.
Target-only views in events, audit records, or UI projections are derived from
that plan. The image target does not change the baseline session model unless
the user separately requests a session model change.

### 3. Image generation compiles to an execution plan

`generate_image` dispatch produces a typed execution plan. The shared contract is
the Meerkat tool. The execution plan is allowed to be provider-specific.

```rust
enum GenerateImageExecutionPlan {
    OpenAiHostedResponsesImageTool {
        max_count: NonZeroU32,
        capabilities: ImageGenerationTargetCapabilities,
        plan: OpenAiResponsesImagePlan,
    },
    OpenAiImagesApi {
        model: ModelId,
        max_count: NonZeroU32,
        capabilities: ImageGenerationTargetCapabilities,
        plan: OpenAiImagesApiPlan,
    },
    GeminiNativeImageModel {
        model: ModelId,
        max_count: NonZeroU32,
        capabilities: ImageGenerationTargetCapabilities,
        plan: GeminiImageTurnPlan,
    },
}
```

This is where Meerkat pays the cost of the OpenAI/Gemini asymmetry. The cost is
paid once, inside the privileged image generation executor, instead of being
spread across prompts, provider adapters, surfaces, or model-visible workflows.

The resolved execution plan is the authority for provider, backend, model target,
maximum count, and capability facts. OpenAI variants project `ProviderId::OpenAi`;
the Gemini variant projects `ProviderId::Gemini`. No parallel
target struct, provider field, backend field, or model-disposition field is
stored beside the plan.

Plan resolution is machine-owned. The resolver may live behind a formal
`ImageExecutionPlanResolver` seam so provider/profile data can be queried
outside the DSL kernel, but the result is accepted by `MeerkatMachine` through a
typed transition. Shell code does not choose OpenAI versus Gemini, clamp counts,
or apply cost/capability policy by constructing a plan on its own.

### 4. OpenAI is a provider-native image backend

For OpenAI image targets, `generate_image` executes through an OpenAI
provider-native image path. That backend may use the Responses hosted
`image_generation` tool or the Images API, depending on the target and provider
contract.

The important semantic rule is negative: OpenAI image generation does not switch
the Meerkat session LLM identity to `gpt-image-*`. GPT Image models are image
generation targets behind the Meerkat tool, not normal chat-session model
identities.

OpenAI execution:

```text
current session model
  -> generate_image(intent, target = OpenAI ...)
  -> provider-native OpenAI image generation
  -> normalized ImageGenerationToolResult
  -> tool result returned to current session model
```

### 5. Gemini is a scoped image-model turn backend

For Gemini image targets, `generate_image` executes by temporarily running a
Gemini native image-output model as part of the tool dispatch.

This uses the same internal scoped-model substrate that explicit `switch_turn`
control intents use, but `generate_image` does not call a public `switch_turn`
tool. The shared primitive is machine-owned runtime state, not a
model-facing ritual.

Gemini execution:

```text
current session model
  -> generate_image(intent, target = Gemini image model)
  -> MeerkatMachine applies operation-scoped model override
  -> runtime lowers current context + typed image intent to Gemini image input
  -> Gemini image model returns text/image assistant content
  -> runtime normalizes output to ImageGenerationToolResult
  -> MeerkatMachine restores baseline session model
  -> tool result returned to original session model
```

The original model then continues the ordinary tool loop. It may accept the
image, ask for another image, revise the request, or produce the final assistant
message.

Gemini text emitted alongside image parts is not silently discarded and is not
automatically appended as user-visible assistant prose. It is normalized into
the image operation result as provider text/caption material and exposed to the
original model in the `generate_image` tool result. The original model decides
whether that text becomes part of the final assistant message. Full provider
text and native metadata remain available in operation audit data and, where
supported, future same-provider projection.

### 6. Projection is typed lowering, not prompt folklore

The Gemini path must not replay the raw `generate_image` function call and hope
the target image model infers what happened. Gemini image models may not support
function calling. The provider-visible request must therefore be normal Gemini
image-generation input derived from Meerkat state.

The projection is a formal lowering step:

```rust
struct ImageTurnProjection {
    operation_id: ImageOperationId,
    target_model: ModelId,
    transcript_context: TranscriptProjection,
    image_intent: ImageGenerationIntent,
    source_images: Vec<ProjectedImageInput>,
    provider_options: ProviderImageOptions,
}

enum ProviderImageOptions {
    OpenAi(OpenAiImageOptions),
    Gemini(GeminiImageOptions),
}
```

For the simple case where the user says, "Based on our discussion draw an
elephant shaped diagram using Nano Banana," the projection may be nearly
equivalent to switching to the Gemini image model and running the turn over the
conversation context.

The projection still needs the typed image intent because the active model may
have done meaningful work before calling `generate_image`: tool calls, file
reads, search, reasoning, target disambiguation, or prompt refinement. At the
dispatch boundary, the `GenerateImageRequest` is the current model's explicit
distillation of what should be generated.

Projection owns:

- which committed transcript context is sent to the target
- which current-turn tool results before `generate_image` are included
- how the typed image intent becomes provider-visible text/content
- which provider-native metadata is preserved
- which unsupported artifacts are dropped or degraded
- which custom tools or built-ins are visible to the scoped target model

Provider adapters consume the projection result. They do not independently
decide transcript replay policy.

Projection is snapshotted when the scoped operation is activated. Compaction,
transcript pruning, or summarization that occurs while a scoped Gemini image turn
is in flight must not change the provider input for that operation. Later turns
may see the compacted transcript, but the active operation consumes the
projection snapshot recorded for its operation id.

Compaction LLM calls have their own routing identity and do not inherit
`switch_turn` or image-operation scoped overrides. They may use model routing
policy, but that policy is compaction-specific and separate from the session's
assistant-turn effective model.

### 7. `switch_turn` is a first-class control feature

This ADR includes the model-routing control feature whose finite-duration path
is also used underneath Gemini image generation.

`switch_turn` is a control intent, not an ordinary external tool. It requests a
change to model routing for a bounded duration. It does not itself produce
assistant content.

```rust
struct SwitchTurnIntent {
    target_model: ModelId,
    duration: SwitchTurnDuration,
    origin: SwitchTurnOrigin,
}

enum SwitchTurnDuration {
    Finite(FiniteScopedTurnDuration),
    UntilChanged,
}

enum FiniteScopedTurnDuration {
    OneTurn,
    Turns(NonZeroU32),
}

enum SwitchTurnOrigin {
    User {
        reason: SwitchTurnReasonTextDisposition,
    },
    Model {
        reason: SwitchTurnReasonTextDisposition,
    },
    SystemPolicy {
        reason: SwitchTurnPolicyReason,
    },
}

enum SwitchTurnReasonTextDisposition {
    NotProvided,
    Provided(SwitchTurnReasonText),
}

struct SwitchTurnReasonText {
    content: String,
}

enum SwitchTurnPolicyReason {
    BudgetDowngrade,
    SafetyHandoff,
    ReleaseTemporaryHold,
}
```

The runtime validates the requested target model, capability policy, cost
policy, safety policy, and user/session permissions. It may reject the intent or
require user confirmation before applying it.

The duration is explicit. Do not encode permanent or until-changed behavior as
`turns = 0`; that would be compact but dangerous. `UntilChanged` is a distinct
semantic fact and can be gated differently from scoped durations.

User-initiated and model-initiated requests share the same machine transition
family but may have different policy. For example, a user may be allowed to
request `UntilChanged` directly, while a model-initiated `UntilChanged` request
may require explicit user approval.

`SwitchTurnOrigin` owns reason provenance by construction. A user-originated
request cannot carry a system-policy reason, and a system-policy request cannot
smuggle free-form user text as its policy basis. `SwitchTurnReasonText` is a
validated/redactable text newtype.

`SystemPolicy` is reserved for machine/runtime policy decisions such as a
budget-driven downgrade for the next assistant turn, a safety-driven handoff to
a stricter model, or releasing a temporary hold back to the session baseline.

Models express `switch_turn` through a reserved builtin control surface that is
parsed into `SwitchTurnIntent` and routed to `MeerkatMachine`. It may be exposed
to model providers as a tool-like declaration where that is the only practical
expression mechanism, but it is not an MCP/external tool and does not use the
ordinary tool dispatcher. Non-model surfaces may submit the same intent through a
runtime control API.

Representative requests:

- `switch_turn("gemini-3.1-flash-image-preview", Finite(OneTurn))` for an
  explicit image-model turn
- `switch_turn("gpt-5.4-pro", Finite(OneTurn))` for one hard reasoning turn
- `switch_turn("gpt-5.4-mini", Finite(Turns(3)))` for a few cheaper
  continuation turns
- `switch_turn("claude-opus-4-7", UntilChanged)` for a user-approved session
  migration

When accepted, finite `switch_turn` requests apply at the next
machine-approved model-call boundary. Duration accounting is by primary Meerkat
assistant turn, not provider call. `Finite(OneTurn)` means the next admitted
assistant turn runs its primary agent LLM calls under the override. Tool-loop
continuations inside that same assistant turn do not consume extra turns, and
operation-scoped provider calls such as a Gemini image-model call do not
decrement the turn budget. `Finite(Turns(n))` consumes `n` admitted assistant
turns that start under the override.

`UntilChanged` is not a scoped override. It is a `switch_turn` API request that,
after policy and approval, compiles to the persistent session LLM reconfigure
transition family. The approval/audit envelope is shared with finite
`switch_turn`, but the executed topology transition is persistent identity
replacement, not `ScopedModelOverride`.

The transcript/control log records the request, approval or denial, applied
target, duration, restore, and any policy intervention. The baseline routing
identity and active scoped override state are machine-owned facts, not inferred
from the last provider call.

Normal image UX must not require the active model to call `switch_turn` before
`generate_image`. The image tool can use the same substrate internally when an
execution plan requires a scoped image-model turn.

If the target model cannot consume the current transcript or tool surface, the
runtime applies the same projection and capability policy used by scoped image
turns. `switch_turn` changes model routing; it does not grant permission to
smuggle unsupported tools, provider-private artifacts, or stale capability
policy into the new model context.

Control results are typed:

```rust
enum SwitchTurnControlResult {
    Applied {
        request_id: SwitchTurnRequestId,
        target_model: ModelId,
        duration: SwitchTurnDuration,
    },
    AwaitingApproval {
        approval_id: ApprovalId,
        target_model: ModelId,
        duration: SwitchTurnDuration,
        reason: SwitchTurnApprovalReason,
    },
    Denied {
        request_id: SwitchTurnRequestId,
        reason: SwitchTurnDenialReason,
    },
}

enum SwitchTurnDenialReason {
    UnsupportedModel,
    CapabilityPolicy,
    CostPolicy,
    SafetyPolicy,
    ApprovalRequiredButUnavailable,
    DeniedDuringApproval {
        approvable: SwitchTurnApprovalReason,
    },
    ScopedOverrideConflict,
    RealtimeTransportConflict,
    ProjectionUnsupported,
}

enum SwitchTurnApprovalReason {
    CrossProvider,
    CostExceedsThreshold,
    SafetyHold,
    UntilChangedFromModelOrigin,
    RealtimeDetachRequired,
}

enum ImageOperationApprovalReason {
    CrossProvider,
    CostExceedsThreshold,
    SafetyHold,
    RealtimeDetachRequired,
}

enum ModelRoutingApprovalTerminalClass {
    Approved,
    DeniedByUser,
    Expired,
    Interrupted,
    SessionArchived,
    SurfaceDetached,
}

enum ModelRoutingApprovalPhase {
    Pending,
    PresentedToUser,
    Terminal(ModelRoutingApprovalTerminalClass),
}

enum ModelRoutingApprovalRequest {
    SwitchTurn {
        request_id: SwitchTurnRequestId,
        reason: SwitchTurnApprovalReason,
    },
    ImageOperation {
        operation_id: ImageOperationId,
        reason: ImageOperationApprovalReason,
    },
}

enum SwitchTurnPhase {
    Requested,
    Validating,
    PendingForBoundary,
    AwaitingApproval(ApprovalId),
    ApplyingFiniteOverride,
    ApplyingPersistentReconfigure,
    ActiveFiniteOverride,
    RestoringFiniteOverride,
    Terminal(SwitchTurnTerminalClass),
}

enum SwitchTurnTerminalClass {
    Denied(SwitchTurnDenialReason),
    ConsumedAndRestored,
    InterruptedAndRestored,
    PersistentReconfigureApplied,
    PersistentReconfigureFailed,
    RestoreFailed { trigger: SwitchTurnRestoreTrigger },
}

enum SwitchTurnRestoreTrigger {
    Consumed,
    Interrupted,
}
```

For a model-initiated request, `Applied` or `Denied` is returned through the
reserved builtin control-result path. `AwaitingApproval` is a machine-owned
suspended turn state with a surfaced `approval_id`. Dispatching surfaces declare
whether they support approval suspension. If the dispatching surface lacks that
capability, the machine denies with `ApprovalRequiredButUnavailable`; the
surface does not fabricate the denial terminal class.

`SwitchTurnControlResult` is the synchronous response to the reserved control
surface. `SwitchTurnPhase` is the lifecycle authority. A synchronous
`AwaitingApproval` result corresponds to `SwitchTurnPhase::AwaitingApproval`;
later approval terminalization drives the switch-turn phase to either a denied
terminal class, finite override application, or persistent reconfigure handoff.
`PersistentReconfigureApplied` means the switch-turn request successfully handed
off to the persistent reconfigure transition family; surfaces that need to know
when the new model is live observe the persistent reconfigure status, not the
switch-turn request id.

`SwitchTurnPhase::Validating` is the pre-decision capability, policy, cost,
safety, transcript-projection, and approval check. It transitions to
`AwaitingApproval`, `PendingForBoundary`, `ApplyingFiniteOverride`,
`ApplyingPersistentReconfigure`, or `Terminal(Denied(...))`.

`SwitchTurnPhase::PendingForBoundary` means the request has been accepted by the
machine but cannot apply until the next eligible model-call boundary. The public
`pending_switch_turn` projection is derived from this phase. Approval-suspended
requests are represented by `SwitchTurnPhase::AwaitingApproval` plus
`approval_phase`, not by helper-local queues.

`ModelRoutingApprovalPhase::Pending` means the machine has created the approval
obligation but no capable surface has acknowledged presentation yet.
`PresentedToUser` means a capable surface has accepted responsibility for the
approval interaction and approval TTL/deadline policy may begin from that
machine transition.

The approval substrate is shared by switch-turn and image-operation policy
holds. `ModelRoutingApprovalRequest` records the parent request and its
parent-specific approval reason directly, so approval terminalization does not
require runtime scans, surface-local lookup, or pairing a parent with an invalid
reason. The approval phase itself remains only the approval lifecycle.

Approval policy maps approvable holds to denial classes when approval is not
available. For example, `RealtimeDetachRequired` may become
the owner-specific realtime transport denial, `CostExceedsThreshold` may become
`CostPolicy`, `SafetyHold` may become `SafetyPolicy`, and `CrossProvider` or
`UntilChangedFromModelOrigin` may become `ApprovalRequiredButUnavailable` when
the dispatching surface cannot suspend for approval. If a user is asked and
refuses, the parent request terminalizes as
`DeniedDuringApproval { approvable }` so audit can distinguish explicit refusal
from automatic policy denial.

Approval terminalization drives explicit parent transitions:

- `ApprovalTerminalized(approval_id, Approved)` moves a switch-turn parent from
  `AwaitingApproval` to `ApplyingFiniteOverride` or
  `ApplyingPersistentReconfigure`, based on `SwitchTurnDuration`.
- `ApprovalTerminalized(approval_id, DeniedByUser)` moves the parent request to
  `Terminal(Denied(DeniedDuringApproval { approvable }))`.
- `ApprovalTerminalized(approval_id, Expired | Interrupted | SessionArchived |
  SurfaceDetached)` moves the parent request to a typed denied terminal selected
  by machine policy. The surface reports the approval outcome; it does not pick
  the parent terminal class.
- For image-operation parents, approval drives
  `ImageOperationPhase::AwaitingApproval` to plan resolution on approval, or to
  `Terminal(Denied(...))` on denial, expiry, interrupt, archive, or surface
  detach.

### 8. Scoped model override is the shared internal substrate

`generate_image` and `switch_turn` share a lower-level runtime primitive:

```rust
struct ScopedModelOverride {
    id: ScopedModelOverrideId,
    kind: ScopedModelOverrideKind,
    previous_effective_model: ModelId,
    baseline_model_snapshot: ModelId,
    target_model: ModelId,
    topology_epoch: TopologyEpoch,
}

enum ScopedModelOverrideKind {
    ImageOperation {
        operation_id: ImageOperationId,
    },
    FiniteSwitchTurn {
        request_id: SwitchTurnRequestId,
        duration: FiniteScopedTurnDuration,
    },
}

```

`generate_image` uses `ScopedModelOverrideKind::ImageOperation` for execution
plans that need a scoped image-model turn. OpenAI image generation does not use
this path.

Finite `switch_turn` uses `ScopedModelOverrideKind::FiniteSwitchTurn` with the
same `FiniteScopedTurnDuration` carried by `SwitchTurnDuration::Finite`.
`UntilChanged` does not create a
`ScopedModelOverride`; it compiles to the persistent reconfigure transition
family after policy and approval. This keeps the implementation DRY without
making image generation depend on a public model-switching ceremony.

Override provenance is derived from `kind` plus the owning image operation or
switch-turn request record. The override does not store a second reason field
that can disagree with the owner.

`baseline_model_snapshot` is audit context captured at override activation. The
authoritative baseline remains session state owned by the machine.

This is a sibling of the existing persistent `reconfigure_live_topology` flow,
not a direct invocation of today's method. The current method is shaped around
realtime attachment authority and persistent session LLM identity replacement.
Scoped overrides need operation/turn-bounded restore semantics and must not
persist the image model as the session baseline.

The sibling must share the same topology invariants:

- topology mutations are serialized by a machine-owned phase/epoch
- every provider binding, tool visibility projection, and realtime callback is
  associated with a topology epoch or fence token
- stale provider callbacks and stale tool-surface revisions are rejected
- effective model identity changes only at machine-approved boundaries
- capability-derived policy and tool visibility are recomputed from the
  effective model identity
- restoration is a machine transition, not shell cleanup
- terminal success is not reported until required restoration has completed

The implementation target is a generalized topology authority where persistent
LLM reconfigure and scoped model override are two transition families. Until that
generalization exists, the ADR forbids adding an independent helper-local scoped
override path.

`TopologyEpoch` is intended to unify with the realtime attachment authority epoch
and fence-token model, not create a parallel epoch identity. The exact type may
change, but it must be the same coherence lineage used to reject stale realtime
callbacks, stale provider bindings, and stale tool-surface projections.
The session-level `topology_epoch` is current authority and is the only value
that advances. The `topology_epoch` stored on `ScopedModelOverride` is the
activation snapshot used to reject stale callbacks and provider results.

All scoped overrides restore the previous effective model on owner terminal. The
machine restores when the owning operation or assistant turn reaches any
terminal class: success, failure, cancel, interrupt, timeout, or provider error.

If restoration itself fails, the machine records a distinct restore-failed
terminal class and exposes a topology-repair-required status. Image operation
restore failures carry `PostActivationImageTerminal` so audit can distinguish, for
example, "cancelled, then restore failed" from "generated, then restore failed."
Switch-turn restore failures carry `SwitchTurnRestoreTrigger` for the same
reason.
The machine must not report the image operation, switch turn, or enclosing
assistant turn as successful while the effective model identity is unresolved.

Nesting is intentionally bounded:

- one turn-scoped override may be active at a time
- one operation-scoped child override may run under that turn-scoped override
- operation-scoped overrides may not nest inside other operation-scoped
  overrides
- a Gemini `generate_image` operation inside a user-initiated
  `switch_turn(..., Finite(OneTurn))` is allowed as the operation-scoped child and
  restores to the previous effective model, not necessarily the baseline model
- model-initiated `switch_turn` requested while an operation-scoped override is
  in flight is denied with `ScopedOverrideConflict`
- user/system `switch_turn` requested while an operation-scoped override is in
  flight may be registered by the machine as pending for the next eligible
  boundary, but it must not supersede the active operation from shell code
- a second finite `switch_turn` while a turn-scoped override is active is denied
  with `ScopedOverrideConflict`
- `switch_turn` requested while persistent reconfigure is in flight is denied or
  machine-queued by persistent reconfigure policy; shell code must not race the
  two topology transitions

`ScopedModelOverrideKind::ImageOperation` is valid only for synchronous,
turn-bound image operations. Detached or background image operations require a
separate lifecycle design because the restore obligation must remain tied to a
live machine-owned turn boundary.

Output ownership differs by scope. Operation-scoped overrides, such as
Gemini-backed `generate_image`, produce operation results; their scoped target
outputs are not canonical session assistant turns. Turn-scoped overrides, such
as `switch_turn("gpt-5.4-pro", Finite(OneTurn))`, route the primary assistant turn to
the target model; that target model directly produces the assistant turn. The
`switch_turn` control intent itself produces only control results.

### 9. MeerkatMachine owns the semantics

Any scoped model override, switch-turn lifecycle, image operation lifecycle, or
tool completion class belongs to `MeerkatMachine` or a formal seam owned by it.

The machine owns:

- image operation id and lifecycle
- switch-turn request id and lifecycle
- model-routing approval id and suspended-request lifecycle
- selected image execution plan, including target
- baseline model identity
- active turn-scoped and operation-scoped model identities, when any
- previous effective model identity for scoped restore
- activation boundary
- remaining scoped turn budget
- topology epoch / fence token for provider and tool-surface coherence
- restore obligation
- capability-derived tool surface revision
- terminal result class

Shell code may perform mechanics: provider calls, blob writes, retries,
streaming events, and persistence. Shell code must not own the semantic truth of
whether the session is currently under a scoped image-model override or whether
the baseline model has been restored.

This is a MeerkatMachine DSL change in
`meerkat-machine-schema/src/catalog/dsl/meerkat_machine.rs`, not a handwritten
runtime authority. No new canonical machine is introduced. The image feature
extends the existing turn execution, ops lifecycle, and live-topology fragments
with fields such as:

```rust
image_operation_phase: Map<ImageOperationId, ImageOperationPhase>,
image_operation_request: Map<ImageOperationId, GenerateImageRequest>,
image_operation_plan: Map<ImageOperationId, GenerateImageExecutionPlan>,
image_projection_snapshot: Map<ImageOperationId, ProjectionSnapshotId>,
switch_turn_phase: Map<SwitchTurnRequestId, SwitchTurnPhase>,
switch_turn_intent: Map<SwitchTurnRequestId, SwitchTurnIntent>,
approval_phase: Map<ApprovalId, ModelRoutingApprovalPhase>,
approval_request: Map<ApprovalId, ModelRoutingApprovalRequest>,
scoped_model_overrides: Map<ScopedModelOverrideId, ScopedModelOverride>,
active_turn_scoped_override: Option<ScopedModelOverrideId>,
active_operation_scoped_override: Option<ScopedModelOverrideId>,
topology_epoch: TopologyEpoch,
```

Exact field names may change during implementation, but the semantic fields
must be catalog DSL fields or derived projections over DSL fields. Optional
active-override fields express a closed absence/presence fact only:
there is either no active override in that slot, or the id resolves through
`scoped_model_overrides`. They must not be used to represent uncertain ownership.
Original intents are stored as machine state for replay, audit, and recovery;
the resolved plan is not the only record of what was requested.
Approval cross-references are machine invariants: if
`switch_turn_phase[request_id] == AwaitingApproval(approval_id)`, then
`approval_request[approval_id]` must be
`ModelRoutingApprovalRequest::SwitchTurn { request_id, .. }`; if
`image_operation_phase[operation_id] == AwaitingApproval(approval_id)`, then
`approval_request[approval_id]` must be
`ModelRoutingApprovalRequest::ImageOperation { operation_id, .. }`.
Closed-world effects must include every new cross-boundary obligation. Model
routing command effects are imperative effects emitted by the machine for shell
mechanics:

```rust
enum ModelRoutingCommandEffect {
    ResolveImageExecutionPlan(ImageOperationId),
    InvokeImageProvider(ImageOperationId),
    CommitGeneratedImageBlob(ImageOperationId),
    ApplyScopedOverride(ScopedModelOverrideId),
    RestoreScopedOverride(ScopedModelOverrideId),
    PresentModelRoutingApproval(ApprovalId),
}
```

Model routing observation effects are past-tense facts reported through typed
machine inputs and acknowledged by machine transitions:

```rust
enum ModelRoutingObservationEffect {
    ImageExecutionPlanResolved(ImageOperationId),
    ProjectionSnapshotted(ImageOperationId),
    ImageProviderResultCaptured(ImageOperationId),
    GeneratedImageBlobCommitted(ImageOperationId),
    ScopedOverrideApplied(ScopedModelOverrideId),
    ScopedOverrideRestored(ScopedModelOverrideId),
    ScopedRestoreFailed(ScopedModelOverrideId),
    ApprovalPresented(ApprovalId),
    ApprovalTerminalized(ApprovalId, ModelRoutingApprovalTerminalClass),
    ImageOperationTerminalized(ImageOperationId, ImageOperationTerminalClass),
    SwitchTurnTerminalized(SwitchTurnRequestId, SwitchTurnTerminalClass),
}
```

The scoped-override observations do not create a second scoped-override
lifecycle store. They are typed evidence consumed by owner transitions: apply
observations set the active override pointer and advance the owning image
operation or switch-turn phase; restore observations clear the active pointer and
advance the owner to its restored terminal; restore-failed observations advance
the owner to its restore-failed terminal. The owner phase plus active pointer is
the authority.

Terminal classes are extracted from `Phase::Terminal(class)` variants; they are
not stored in parallel terminal maps. The machine-codegen, drift-check, and TLC
verification cycle is required for these changes.

Identity follows the existing identity-first model. Preferences that survive mob
member respawn, such as per-member default image target, are keyed by
`AgentIdentity`. Active scoped overrides are per runtime binding and are guarded
by `AgentRuntimeId` plus `FenceToken`. `TopologyEpoch` advancement must respect
existing `FenceToken` ordering so stale provider callbacks, stale runtime
bindings, and stale tool-surface projections cannot mutate current state.

### 10. `generate_image` is privileged builtin dispatch

`generate_image` is not an ordinary MCP/external tool. Ordinary tool dispatch
cannot be responsible for live model topology, transcript projection, or
provider capability policy.

The builtin is modeled as an image-operation fragment of `MeerkatMachine`, not
as an informal shell sequence:

```rust
enum ImageOperationPhase {
    Requested,
    Validating,
    AwaitingApproval(ApprovalId),
    PlanResolved,
    ProjectionSnapshotted,
    ScopedOverrideActive,
    ProviderCallInFlight,
    ProviderResultCaptured,
    BlobCommitPending,
    ResultCommitted,
    RestoringScopedOverride,
    Terminal(ImageOperationTerminalClass),
}

enum ImageOperationTerminalClass {
    Generated,
    EmptyResult { provider_text: ProviderTextDisposition },
    Denied(ImageOperationDenialReason),
    RefusedByProvider,
    SafetyFiltered,
    Failed,
    Cancelled,
    Timeout,
    ScopedRestoreFailed { trigger: PostActivationImageTerminal },
}

enum ImageOperationDenialReason {
    UnsupportedTarget,
    UnsupportedCount,
    CapabilityPolicy,
    CostPolicy,
    SafetyPolicy,
    ApprovalRequiredButUnavailable,
    DeniedDuringApproval {
        approvable: ImageOperationApprovalReason,
    },
    ScopedOverrideConflict,
    RealtimeTransportConflict,
    ProjectionUnsupported,
}

enum PostActivationImageTerminal {
    Generated,
    EmptyResult,
    Denied(PostActivationImageDenialReason),
    RefusedByProvider,
    SafetyFiltered,
    Failed,
    Cancelled,
    Timeout,
}

enum PostActivationImageDenialReason {
    CostPolicy,
    SafetyPolicy,
    DeniedDuringApproval {
        approvable: ImageOperationApprovalReason,
    },
    ScopedOverrideConflict,
    RealtimeTransportConflict,
    ProjectionUnsupported,
}
```

`ImageOperationPhase::Validating` is the pre-decision target resolution,
capability, policy, cost, safety, projection-feasibility, and approval check. It
transitions to `AwaitingApproval`, `PlanResolved`, or `Terminal(Denied(...))`.

`ImageOperationPhase` is shared, but legal transitions are conditional on
`image_operation_plan`. All plans pass through `ProjectionSnapshotted`; for
OpenAI provider-native plans this snapshot is the provider request/audit
projection, not cross-model transcript replay. OpenAI plans skip
`ScopedOverrideActive` and `RestoringScopedOverride`; Gemini scoped image-model
plans require them. OpenAI plans cannot terminalize as `ScopedRestoreFailed`
because they never activate a scoped override. The DSL must encode these
plan-conditional guards rather than relying on shell code to take the right path.
`PostActivationImageTerminal` is intentionally narrower than
`ImageOperationTerminalClass`; pre-activation denials such as `UnsupportedTarget`
and `UnsupportedCount` cannot be restore triggers because no scoped override has
been applied yet.

Machine transitions own phase changes, target/plan acceptance, projection
snapshot identity, scoped override activation/restoration, and terminal class.
Shell mechanics execute only after the machine emits a closed-world effect such
as `ResolveImageExecutionPlan`, `InvokeImageProvider`, `CommitGeneratedImageBlob`,
or `RestoreScopedOverride`.

The tool result returned to the original model carries
`ImageOperationTerminalClass`. A denied, refused, empty, cancelled, timed out,
safety filtered, or restore-failed operation is not collapsed into a generic
successful tool result with string warnings.

Cancellation of the enclosing assistant turn cancels the in-flight scoped
provider call when the provider path supports cancellation. If the provider call
cannot be cancelled, its eventual output is discarded unless the machine still
recognizes the operation id and topology epoch as current. In both cases the
machine restores scoped model state before terminalizing the operation as
`Cancelled`.

### 11. Assistant image content is canonical, but tool-loop control remains normal

Generated image bytes are stored as blobs and represented in canonical transcript
state through typed image content.

```rust
AssistantBlock::Image {
    image_id,
    blob_ref,
    media_type,
    width,
    height,
    revised_prompt: RevisedPromptDisposition,
    meta: ProviderImageMetadata,
}
```

`image_id: AssistantImageId` is the canonical generated-image identity.
`blob_ref` points to stored bytes. `media_type` is a typed `MediaType` newtype,
not a free string.

The image artifact is linked to the `generate_image` operation and can be
rendered by UI/API surfaces as soon as it is committed. The original model still
receives a compact tool result so it can decide whether to continue, retry, or
finish the assistant message.

```rust
struct ImageGenerationToolResult {
    operation_id: ImageOperationId,
    terminal: ImageOperationTerminalClass,
    images: Vec<AssistantImageRef>,
    provider_text: ProviderTextDisposition,
    revised_prompt: RevisedPromptDisposition,
    warnings: Vec<ImageGenerationWarning>,
    native_metadata: ProviderImageMetadata,
}

enum ProviderTextDisposition {
    NotEmitted,
    UnsupportedByBackend,
    Captured(TextArtifactRef),
}

enum RevisedPromptDisposition {
    NotRequested,
    UnsupportedByBackend,
    Unchanged,
    Revised {
        text: PromptText,
        source: RevisedPromptSource,
    },
}

enum RevisedPromptSource {
    Provider,
    MeerkatProjection,
}

enum ImageGenerationWarning {
    ContinuityDegraded,
    UnsupportedArtifactDropped,
    ProviderReturnedFewerImages {
        requested: NonZeroU32,
        returned: NonZeroU32,
    },
}

enum ProviderImageMetadata {
    NotEmitted,
    OpenAi(OpenAiImageMetadata),
    Gemini(GeminiImageMetadata),
}
```

Provider metadata is a tagged provider-keyed union with provenance, target model,
provider response identifiers, and continuity handles where supported. It is not
an untyped metadata bag and is not canonical generated-image identity.
`ImageGenerationToolResult::terminal` is the model-visible projection of the
machine-owned image operation terminal class. Payload fields are interpreted in
the context of that terminal; for example, `Generated` expects one or more image
refs, while `Denied`, `Cancelled`, `Timeout`, and `ScopedRestoreFailed { .. }`
may carry no images.

`AssistantBlock::Image.meta` is the canonical transcript metadata. The
`ImageGenerationToolResult.native_metadata` field is a delivery-time projection
of that same metadata for the tool result.

`AssistantImageRef` is also a delivery-time projection used so the original model
can inspect the generated image result before or alongside transcript rendering.
The canonical identity and metadata remain on `AssistantBlock::Image`.

`ProviderTextDisposition::Captured` carries text emitted by backends such as
Gemini image models. It is visible to the original model as tool-result context
and retained for audit and provider continuity, but it is not canonical
assistant prose unless the original model emits it in the surrounding assistant
turn.

During an operation-scoped image turn, the session event stream emits typed
scoped-operation lifecycle and image-generation progress events. Provider deltas
from the scoped target are normalized into operation progress, provider text, or
committed image events; they are not emitted as assistant content deltas for the
main session turn.
Event identity is phase-driven: progress events carry `image_operation_id`,
`image_operation_phase`, selected plan, and committed `AssistantImageId` values
when available.

This preserves both facts:

- the image is not an opaque provider-local tool log
- the original model remains in charge of the surrounding assistant turn

### 12. Capabilities distinguish chat models from image targets

Model/profile capability data must distinguish at least:

- `hosted_image_generation_tool`
- `native_image_output`
- `custom_tools`
- `image_search_grounding`
- `image_continuity_tokens`
- provider-native reasoning or transcript-continuity metadata

Do not collapse these into one `assistant_image_output` bit. UI affordances may
derive a simple "can generate images" projection, but runtime policy and request
construction require the precise facts.

### 13. Public model status reports baseline and effective identity

Surfaces must not infer "current model" from the last provider call or from a
single string field. The runtime exposes a typed model-routing projection:

```rust
struct SessionModelRoutingStatus {
    baseline_model: ModelId,
    effective_model: ModelId,
    active_turn_override: Option<ScopedModelOverrideSummary>,
    active_operation_override: Option<ScopedModelOverrideSummary>,
    pending_switch_turn: Option<SwitchTurnRequestSummary>,
}
```

`effective_model` is a derived projection computed from machine state on every
read: active operation-scoped override, else active turn-scoped override, else
baseline model. It is not stored as a separate authority and must not be cached
by shell helpers as semantic truth.
That precedence order is part of the typed projection contract, not a UI
convention.

At most one pending switch-turn request may exist per session. A second pending
request is denied or replaces the first only through a machine transition; shell
queues do not own pending switch-turn truth.

Legacy surfaces that can only show one model should show `baseline_model` as the
session model and optionally annotate that a scoped override is active. New
surfaces should display both baseline and effective identity when they differ.

Mob implications follow from session ownership: each mob member has its own
session model-routing status. Mob delegation policy that asks "what model can
member X serve?" should read baseline/member profile capability, not a transient
effective model from an in-flight scoped override, unless the policy explicitly
opts into effective routing state.

### 14. Realtime attachment follows effective model identity

Realtime attachment remains capability-driven. The relevant identity during a
scoped override is the effective model identity, not only the baseline model.

If a scoped override targets a model whose capabilities do not support realtime
while a realtime binding is attached, the runtime must either:

- reject the override before activation with the owner-specific realtime denial
  class (`SwitchTurnDenialReason::RealtimeTransportConflict` for switch-turn
  owners, `ImageOperationDenialReason::RealtimeTransportConflict` for image
  operation owners), or
- detach realtime for the scoped duration and reattach after restoration if the
  restored effective model supports realtime.

The choice is policy-controlled, but it is not surface-local. Provider callback
authority epochs, topology epochs, and realtime binding state must reject stale
callbacks across the detach/reattach boundary. The runtime must never keep a
realtime transport attached to a non-realtime effective model.

The deny-versus-detach policy is owned by the runtime model-routing policy
resolver at the same override-first composition seam that resolves session,
mob/member, and organization policy. Surfaces and mob delegation code may
present policy-derived status, but they do not store or decide this policy
independently.

For example, if a realtime-capable turn-scoped override is active and a nested
Gemini image operation targets a non-realtime model, the operation-scoped
effective identity triggers the same deny-or-detach policy. Restoration returns
to the turn-scoped effective model and realtime is reattached only if that
restored effective model supports realtime.

## Rejected Alternatives

### Provider-specific image tools

Expose `openai_generate_image`, `gemini_generate_image`, and similar tools.

Rejected because it leaks provider execution shape to the active model and
creates duplicate policy, UX, and audit paths.

### Ask the model to call `switch_turn` before `generate_image`

Rejected because normal image UX should be one operation. A user asks for an
image; the model should express an image-generation intent, not manually perform
runtime choreography.

### Replay the raw tool call to Gemini image models

Rejected because Gemini image targets may not support function calling. The
provider-visible Gemini request must be projected image-generation content, not a
raw Meerkat tool call.

### Always fork an ephemeral image sub-session

Rejected as the default because Gemini text-to-Gemini image requests should be
able to preserve same-provider context and metadata where supported. A fork may
remain useful for specific cross-provider or privacy policies, but it is not the
primary model.

### Let provider adapters decide transcript projection

Rejected because replay policy would become shadow truth. Projection is a
Meerkat contract consumed by provider adapters.

## Consequences

### Positive

- Users and models get one stable `generate_image` operation.
- OpenAI and Gemini keep their clean provider-native paths.
- The asymmetry is explicit and localized in the execution planner.
- Explicit `switch_turn` and implicit image-generation switches share reusable
  infrastructure without making image UX depend on model-routing choreography.
- Gemini same-provider image generation can preserve richer context when
  supported.
- Provider adapters execute mechanics instead of owning replay semantics.
- The original model remains in the ordinary tool loop and can iterate.
- Future model-routing UX has typed duration, validation, restore, and audit
  semantics from the start.

### Negative

- `generate_image` must be privileged builtin dispatch, not a normal external
  tool.
- The runtime needs machine-owned scoped model override state.
- `switch_turn` introduces a new control surface with policy, approval, audit,
  and transcript-projection implications.
- Projection becomes a correctness-critical layer and needs provider contract
  tests.
- The Gemini path is more complex than the OpenAI path.
- The tool loop needs to tolerate a provider call that temporarily changes model
  topology inside tool dispatch.
- Dynamic tool visibility and provider policy must be recomputed whenever a
  scoped override changes the active model identity.

## Constraints

- Do not model OpenAI `gpt-image-*` as normal Meerkat chat-session LLM
  identities.
- Do not conflate session model identity with image generation target.
- Do not expose provider-native image APIs as the model-facing contract when the
  Meerkat-owned `generate_image` tool is enabled.
- Do not let shell code own image target resolution, execution-plan selection,
  count clamping, or cost/capability policy.
- Do not store provider, backend, model target, or capability facts beside
  `GenerateImageExecutionPlan`; those are projections from the plan variant.
- Do not silently clamp image `count`; reject unsupported counts through a
  machine-owned terminal class.
- Do not add an `Ask` variant to `ImageGenerationTargetPreference`; approval and
  denial are machine policy and terminalization, not model-facing target
  selectors.
- Do not dispatch `generate_image` as an ordinary external tool.
- Do not model `switch_turn` as an ordinary external tool with helper-local
  routing state.
- Do not encode `UntilChanged` or permanent switching as `turns = 0`.
- Do not execute `UntilChanged` through `ScopedModelOverride`; route it through
  the persistent reconfigure transition family after policy and approval.
- Do not create speculative scoped override kinds; every closed-world kind
  variant must have a present transition family and caller.
- Do not represent scoped override owner, duration, or reason as independent
  fields that can disagree. `ScopedModelOverrideKind` owns the by-construction
  shape.
- Do not let `switch_turn` bypass model capability, tool visibility, safety,
  cost, or user-approval policy.
- Do not count provider calls or tool-loop continuations as separate
  `switch_turn` turns; finite switch durations count admitted assistant turns.
- Do not let compaction inherit assistant-turn scoped overrides or mutate the
  projection snapshot of an active scoped operation.
- Do not make the target Gemini image model consume raw Meerkat function-call
  syntax.
- Do not let provider adapters invent transcript projection policy.
- Do not store provider inline image bytes as durable transcript truth.
- Do not hide scoped model ownership in helper-local flags or `Option<T>`.
- Do not apply scoped model overrides outside machine-owned transitions.
- Do not create a scoped override path that bypasses the generalized topology
  epoch/fence invariants shared with persistent live-topology reconfigure.
- Do not add image-operation, scoped-override, approval, or switch-turn terminal
  classes outside the closed-world DSL/effect catalog.
- Do not store terminal classes in parallel maps; terminal classes live inside
  `Phase::Terminal(class)` and are projected from phase.
- Do not mix command effects and observation effects under one name; machine to
  shell commands are imperative, shell observations are past-tense facts.
- Do not drop original request intents after plan resolution. The machine stores
  both `GenerateImageRequest` and `SwitchTurnIntent` for replay, audit, and
  recovery.
- Do not discover approval parents by surface state or runtime scans;
  `approval_request` is machine state and stores the parent-specific reason.
- Do not store a parallel scoped-override phase unless scoped overrides become
  their own lifecycle owner. In this ADR, the owner phase plus active override
  pointer is the authority.
- Do not nest operation-scoped overrides.
- Do not allow more than one `PendingForBoundary` switch-turn request per
  session except by a machine-owned replace/deny transition.
- Do not use `ScopedModelOverrideKind::ImageOperation` for detached/background
  operations.
- Do not preserve custom tools for a scoped target model whose capabilities say
  custom tools are unsupported.
- Do not keep realtime transport attached to a non-realtime effective model.
- Do not expose a single ambiguous "current model" projection once scoped
  overrides exist; expose baseline and effective model identities.
- Do not reverse the model-routing projection precedence; effective model is
  operation-scoped override, then turn-scoped override, then baseline.
- Do not emit scoped image-model provider deltas as main-session assistant
  content deltas.
- Do not commit output from a cancelled scoped provider call unless the machine
  still recognizes the operation id and topology epoch as current.
- Do not add provider image option or metadata catch-all variants. Adding a new
  image provider requires closed-world updates to provider option/metadata
  unions, capability rows, projection, and machine effects.
- Dynamic policy and tool surface visibility must be recomputed after every
  scoped model identity change.

## Follow-on Work

- Define `GenerateImageRequest`, `ImageGenerationIntent`, and target preference
  types.
- Define `SwitchTurnIntent`, `SwitchTurnDuration`, and runtime control result
  types.
- Define `ImageOperationPhase`, `ImageOperationTerminalClass`, and
  `SwitchTurnPhase` / `SwitchTurnTerminalClass` as DSL-owned closed sets.
- Define `ImageSourceRef`, `ImageContinuityRef`, and provider-native image
  handle fallback rules.
- Define `ImageGenerationTargetCapabilities` and
  `ImageContinuityTokenSupport`.
- Define `SwitchTurnApprovalReason`, `ImageOperationApprovalReason`,
  `ModelRoutingApprovalRequest`, and approval suspension terminal states.
- Define `ScopedModelOverrideSummary` and `SwitchTurnRequestSummary` projection
  types.
- Add `MediaType`/prompt provenance newtypes or adopt existing typed media
  wrappers for assistant image blocks.
- Add a surface capability flag for approval suspension support.
- Add wire types to `meerkat-contracts` and make schema regeneration part of
  the implementation checklist.
- Add image target capability/profile rows for OpenAI and Gemini.
- Add `generate_image` as a Meerkat-owned builtin tool.
- Add `switch_turn` as a machine-owned control intent/control surface.
- Add privileged image generation executor and machine-owned execution-plan
  resolution seam.
- Add OpenAI provider-native image backend plans.
- Add Gemini scoped image-model backend plans.
- Add machine-owned scoped model override fields/transitions.
- Add MeerkatMachine DSL fields for image operations, switch-turn lifecycle,
  projection snapshots, approvals, and scoped overrides.
- Add closed-world DSL effects for image plan resolution, projection snapshots,
  scoped override apply/restore, approval suspension/terminalization, image
  commit, and terminalization; run machine-codegen, drift-check, and TLC
  verification.
- Add approval, denial, applied, and restored event records for scoped model
  overrides.
- Generalize live topology reconfigure into a topology authority shared by
  persistent reconfigure and scoped overrides.
- Add topology epoch/fence-token invariants for scoped overrides and stale
  provider/tool-surface rejection.
- Add bounded nesting tests for turn-scoped plus operation-scoped overrides.
- Add tests that `UntilChanged` routes through persistent reconfigure rather
  than scoped override.
- Add compaction tests for projection snapshots and compactor model routing.
- Add model-routing status projection with baseline and effective identities.
- Add mob policy tests that distinguish member baseline/profile capability from
  transient effective model state.
- Add realtime detach/reattach or denial policy for scoped overrides targeting
  non-realtime models.
- Add transcript/image turn projection contracts.
- Add blob-store integration for generated images.
- Add archive/retention policy for assistant image blobs on session archive and
  deletion.
- Add `AssistantBlock::Image` and image streaming events.
- Add scoped-operation lifecycle and image progress streaming events.
- Add provider contract tests for OpenAI native image generation.
- Add provider contract tests for Gemini scoped image-model turns.
- Add cross-provider degradation tests.

## External References

- [OpenAI Responses image generation tool](https://developers.openai.com/api/docs/guides/tools-image-generation)
- [OpenAI image generation guide](https://developers.openai.com/api/docs/guides/image-generation?api=responses)
- [OpenAI reasoning and replay guidance](https://developers.openai.com/api/docs/guides/reasoning)
- [Gemini image generation guide](https://ai.google.dev/gemini-api/docs/image-generation)
- [Gemini 3 thought signatures](https://ai.google.dev/gemini-api/docs/gemini-3#thought-signatures)
- [Gemini 3.1 Flash Image Preview model page](https://ai.google.dev/gemini-api/docs/models/gemini-3.1-flash-image-preview)
- [Gemini 3 Pro Image Preview model page](https://ai.google.dev/gemini-api/docs/models/gemini-3-pro-image-preview)

## Related Material

- [Meerkat Runtime Dogma](/architecture/meerkat-runtime-dogma)
- [Formal Seam Closure](/architecture/formal-seam-closure)
- [Realtime Transcript Fidelity](/architecture/realtime-transcript-fidelity)
- [Architecture hub](/reference/architecture)
