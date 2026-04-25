# Wave (b) — Correct Foundations — Implementation Plan

**Branch of record:** `dogma/wave-a-demolition` @ `7f88cb477`. Tree is broken post-wave-a. Foundations below are verified at HEAD.

---

## Section 1 — Foundation inventory

### V1. Machine kernel & schema typing

**Current state.**
- `meerkat-machine-kernels/src/runtime.rs:134-163`: `KernelState.phase: String`, `KernelInput.variant: String`, `KernelSignal.variant: String`, `KernelEffect.variant: String`, `TransitionOutcome.transition: String`, fields keyed by `BTreeMap<String, KernelValue>`.
- `meerkat-machine-kernels/src/runtime.rs:165-204`: every `TransitionRefusal` variant carries string `machine`/`variant`/`phase`.
- `meerkat-machine-schema/src/machine.rs:228` (`EffectDisposition::Routed { consumer_machines: Vec<String> }`), `:244-246` (`RustBinding.crate_name/module: String`), `:314-318` (`VariantSchema.name: String`), `:338-350` (`TypeRef::Named(String)`, `TypeRef::Enum(String)`), `:370-375` (`TransitionSchema.name/from/to: String`), `:386-391` (`TriggerMatch.variant: String`), `:401-404` (`EffectEmit.variant: String`), `:407-419` (`Update::{Assign,Increment,Decrement}.field: String`).
- `meerkat-machine-schema/src/composition.rs:7-26` (`CompositionSchema.name: String`, `deep_domain_overrides: BTreeMap<String, usize>`), `:29-33` (`ActorSchema.name: String`), `:48-72` (`EffectHandoffProtocol.{name,producer_instance,effect_variant,realizing_actor,correlation_fields,obligation_fields}: String/Vec<String>`), `:102-118` (`FeedbackInputRef.machine_instance/input_variant: String`).
- `meerkat-machine-codegen/src/render.rs:4313-4316`: `render_named_type_alias_target` hard-codes the allow-list `BoundarySequence | TurnNumber | FenceToken | Generation → u64`, all else → `String`. Referenced at `:2996, :3722, :4327, :4393, :4460, :4527`.

**Target typed shape.** Introduce in `meerkat-machine-schema/src/types.rs` (or a new `identity.rs`):

```rust
/// Kernel-level identities — opaque slug-newtypes validated at construction.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MachineId(String);      // e.g. "MobMachine"
pub struct MachineInstanceId(String); // e.g. "mob"
pub struct PhaseId(String);
pub struct InputVariantId(String);
pub struct SignalVariantId(String);
pub struct EffectVariantId(String);
pub struct FieldId(String);
pub struct TransitionId(String);
pub struct RouteId(String);
pub struct ProtocolId(String);
pub struct ActorId(String);
pub struct NamedTypeId(String);
pub struct EnumTypeId(String);
pub struct EnumVariantId(String);

impl MachineId { pub fn parse(s: &str) -> Result<Self, IdentityError>; pub fn as_str(&self) -> &str; }
// identical shape for each newtype; slugs MUST be non-empty ASCII
```

Propagate:
- `KernelState.phase: PhaseId`, `.fields: BTreeMap<FieldId, KernelValue>`.
- `KernelInput.variant: InputVariantId`, `KernelSignal.variant: SignalVariantId`, `KernelEffect.variant: EffectVariantId` (typed fields keyed by `FieldId`).
- `TransitionOutcome.transition: TransitionId`.
- `TransitionRefusal::*` — every `machine`/`variant`/`phase` string → the typed ID.
- `CompositionSchema.name: CompositionId` (new), `.machines: Vec<MachineInstance>` with typed `MachineInstance { instance_id: MachineInstanceId, machine_name: MachineId, actor: ActorId }`.
- `EffectDisposition::Routed { consumer_machines: Vec<MachineId> }`.
- `TransitionSchema { name: TransitionId, from: Vec<PhaseId>, to: PhaseId, on: TriggerMatch, updates: Vec<Update>, emit: Vec<EffectEmit> }`.
- `Update::Assign { field: FieldId, expr: Expr }`, `Increment/Decrement { field: FieldId, amount: u64 }`.
- `EffectEmit { variant: EffectVariantId, fields: IndexMap<FieldId, Expr> }`.
- `TriggerMatch { kind: TriggerKind, variant: InputVariantId | SignalVariantId, bindings: Vec<FieldId> }` — split `TriggerMatch` into `InputTriggerMatch` / `SignalTriggerMatch` or tag the variant via `TriggerKind` enum, but the *variant id* must be typed at the right level.
- `TypeRef::Named(NamedTypeId)`, `TypeRef::Enum(EnumTypeId)`.
- `VariantSchema { name: EnumVariantId, fields: Vec<FieldSchema> }`, `FieldSchema { name: FieldId, ty: TypeRef }`.

Kill the `render_named_type_alias_target` allow-list. Replace with an authoritative `NamedTypeBinding` on each `NamedType` declaration in the DSL: `struct NamedTypeBinding { name: NamedTypeId, rust: RustTypeAtom }` where `RustTypeAtom` is a small enum (`U64 | U32 | String | TypePath(String)`). The catalog DSL must declare this binding for every named type; codegen consults the binding, never a string-name match.

**Consumer surface.**
- `meerkat-machine-kernels` (all of `runtime.rs`, `generated/` output).
- `meerkat-machine-codegen/src/render.rs` and `artifacts.rs` (keyed lookups of variants/phases/fields).
- `meerkat-machine-schema/src/catalog/**` — every DSL module reconstructing machines must take typed IDs.
- `meerkat-runtime/src/meerkat_machine/**` — dispatch tables, `dsl.rs` transitions.
- `meerkat-mob/src/machines/mob_machine.rs` and `meerkat-mob/src/mob_machine.rs` — `MobMachineInput` variant-to-kernel mapping.
- `xtask/src/machines.rs`, `machines_tests.rs`, `rmat_policy.rs`, `seam_inventory.rs`, `ownership_ledger.rs`.

**Propagation order.** `meerkat-machine-schema::types` (define newtypes) → `meerkat-machine-schema::machine`, `meerkat-machine-schema::composition` (replace `String` fields) → `meerkat-machine-kernels::runtime` (kernel IDs typed) → `meerkat-machine-codegen::render` (drop `render_named_type_alias_target`; consume `NamedTypeBinding`) → `meerkat-machine-schema::catalog::*` (re-author DSL with typed constructors) → kernel consumers (`meerkat-runtime`, `meerkat-mob`) → xtask (RMAT, seam inventory, ownership ledger audits).

---

### V2. Composition dispatch typing

**Current state.**
- `meerkat-runtime/src/composition_dispatch.rs` and `recompute_mob_peer_overlay.rs` are **deleted** on this branch (wave-a tombstones — `ce2dbe35e`, `f5e366f38`). The catalog stub at `meerkat-machine-schema/src/catalog/compositions.rs:295` now sets `driver: None` (verified). Nothing wires the composition dispatcher into the runtime module tree; `meerkat-runtime/src/lib.rs:18-60` shows no `composition_dispatch` nor `generated::composition` mod.
- `meerkat-machine-codegen/src/artifacts.rs:900-935` (from dogma scan) emitted the original `ProducerInstance: String` + `effect: String` + `payload: serde_json::Value` shape. The emitter still exists but no runtime consumes the emitted module.

**Target typed shape.** In a new `meerkat-runtime/src/composition/` tree (which replaces the deleted `composition_dispatch.rs`):

```rust
/// Typed identity for a producing machine instance inside a composition.
pub struct ProducerInstance {
    pub composition: CompositionId,
    pub instance_id: MachineInstanceId,
    pub machine: MachineId,
}

/// Typed, strongly-typed effect payload. Generic over the producer's effect enum.
/// The codegen emits one `EffectPayload<MachineFooEffect>` wrapper per producer.
pub enum EffectPayload<MachineEffect> {
    Emitted { variant: EffectVariantId, body: MachineEffect },
}

/// Typed route identity.
pub struct RouteKey {
    pub composition: CompositionId,
    pub route_id: RouteId,
}

/// The composition dispatcher IS the execution path for routed effects.
/// No helper functions, no "maybe call driver" branches.
#[async_trait]
pub trait CompositionDispatcher: Send + Sync {
    async fn dispatch<E: ProducerEffect>(
        &self,
        producer: ProducerInstance,
        effect: EffectPayload<E>,
    ) -> Result<DispatchOutcome, DispatchRefusal>;
}
```

The `MeerkatMachine` actor must own a `CompositionDispatcher` handle and call it inline whenever a routed effect is produced — there is no `_maybe_recompute_overlay()` or similar opt-in path. Route resolution (looking up the target machine input from a `RouteKey`) uses typed `RouteId → (MachineInstanceId, InputVariantId, Vec<FieldBinding<FieldId, FieldId>>)` tables generated by `meerkat-machine-codegen` at build time.

Codegen change (`meerkat-machine-codegen/src/artifacts.rs`): replace the string-and-JSON shape with per-composition Rust modules that expose:
```rust
pub enum MeerkatMobSeamEffect {
    Mob(crate::generated::mob_machine::MobEffect),
    Meerkat(crate::generated::meerkat_machine::MeerkatEffect),
}
pub fn route_to_input(effect: &MeerkatMobSeamEffect) -> Option<TypedRoutedInput>;
```

**Dogmatic decision vs the alternative.** The alternative is "keep a helper dispatcher that callers opt into". That is what wave-a deleted. Wave-b rebuilds composition dispatch as *the* runtime execution seam — any routed effect that bypasses the dispatcher is an RMAT violation. This eliminates dogma #75 / #76 / #77 at the vocabulary level because the dispatcher carries typed producer, typed variant, and typed payload from end to end.

**Consumer surface.**
- `meerkat-runtime/src/meerkat_machine/dispatch_*.rs` (all dispatch_drain.rs, dispatch_session.rs, dispatch_control.rs, dispatch_ingress.rs — must route via the dispatcher trait).
- `meerkat-runtime/src/mob_adapter.rs` (currently bridges mob → meerkat signals; must consume typed routed inputs).
- `meerkat-machine-codegen/src/artifacts.rs` (emitter).
- `meerkat-machine-schema/src/catalog/compositions.rs` (re-declare `meerkat_mob_seam` with typed route keys).

**Propagation order.** V1 must land first (typed `RouteId`, `MachineInstanceId`, `InputVariantId`). Then: codegen emits typed per-composition modules → `meerkat-runtime/src/composition/` exposes `CompositionDispatcher` trait + default impl → meerkat-machine dispatch callers consume the dispatcher → schema catalog declares driver bindings typed.

---

### V3. Runtime turn metadata

**Current state.**
- `meerkat-core/src/lifecycle/run_primitive.rs:93-125`: `RuntimeTurnMetadata` carries `model: Option<String>`, `provider: Option<String>`, `provider_params: Option<serde_json::Value>`, `render_metadata: Option<RenderMetadata>`, plus `handling_mode`, `skill_references`, `flow_tool_overlay`, `additional_instructions`, `execution_kind`.
- Missing explicit `connection_ref: Option<ConnectionRef>` field and `keep_alive: Option<KeepAlivePolicy>`. The dogma catalog (#46) says these travel alongside but are dropped; the actual HEAD has `model`, `provider`, `provider_params`, `render_metadata` but **no** `connection_ref` or `keep_alive`. The overrides are incompletely captured: the seam carries scalars that the runtime cannot use authoritatively, and the fields the runtime does need (`connection_ref`, `keep_alive`) are silently dropped.
- `meerkat-runtime/src/runtime_loop.rs:78-99` — `input_turn_metadata` constructs `RuntimeTurnMetadata` from `ExternalEvent`/`Continuation` and otherwise clones `prompt.turn_metadata` / `flow_step.turn_metadata`. Two construction sites.
- `meerkat-runtime/src/runtime_loop.rs:147-193` (`inputs_to_primitive_with_boundary`) merges per-input metadata via `merge_batch_turn_metadata`; the merge rule is scalar-last-wins + collection-accumulating.

**Target typed shape.** In `meerkat-core/src/lifecycle/run_primitive.rs`:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct RuntimeTurnMetadata {
    pub handling_mode: Option<HandlingMode>,
    pub skill_references: Option<Vec<SkillKey>>,          // SkillKey, not SkillId
    pub flow_tool_overlay: Option<TurnToolOverlay>,
    pub additional_instructions: Option<Vec<TurnInstruction>>, // typed, see below
    pub model: Option<ModelId>,                            // newtype
    pub provider: Option<ProviderId>,                      // existing Provider enum
    pub provider_params: Option<ProviderParamsOverride>,   // typed struct, not Value
    pub connection_ref: Option<ConnectionRef>,             // canonical
    pub keep_alive: Option<KeepAlivePolicy>,               // typed enum
    pub render_metadata: Option<RenderMetadata>,
    pub execution_kind: Option<RuntimeExecutionKind>,
}

pub struct ModelId(String);
pub struct KeepAlivePolicy { pub ttl: std::time::Duration, pub policy: KeepAliveMode }
pub enum KeepAliveMode { Pinned, PolicyDriven }
pub struct TurnInstruction { pub kind: TurnInstructionKind, pub body: String }
pub enum TurnInstructionKind { User, System, Host }
pub struct ProviderParamsOverride {
    pub temperature: Option<f32>,
    pub top_p: Option<f32>,
    pub max_output_tokens: Option<u32>,
    pub reasoning: Option<ReasoningMode>,
    pub thinking_budget_tokens: Option<u32>,
    pub provider_tag: Option<ProviderTag>, // carries anthropic/openai/gemini-specific knobs via typed sub-enum
    // NO serde_json::Value
}
```

`provider_params` carries the union of all provider-specific knobs that today escape as `Value`. Anything that doesn't fit a typed field is a provider-specific extension that lives on its provider's per-binding config (authoritative), *not* on the per-turn override.

Single canonical construction site: `RuntimeTurnMetadata::for_input(input: &Input) -> Option<Self>` in `meerkat-runtime/src/runtime_loop.rs` replaces both current construction sites. `merge_batch_turn_metadata` stays but operates on the typed struct with typed merge semantics (scalars: error on conflict rather than last-wins — a batch with two distinct `model` overrides is a programmer error that the runtime must refuse).

**Consumer surface.**
- `meerkat-core/src/lifecycle/run_primitive.rs`, `core_executor.rs`.
- `meerkat-core/src/agent/state.rs`, `runner.rs`, `builder.rs` — every place that today reads `.model`/`.provider`/`.provider_params`.
- `meerkat-runtime/src/runtime_loop.rs`.
- `meerkat/src/surface/runtime_backed.rs:210,214,216,225`, `meerkat/src/surface/runtime_schedule_host.rs:223`.
- `meerkat/src/factory.rs:1331`, `service_factory.rs:183,193,202`.
- `meerkat-session/src/persistent.rs:1251,1448` (persisted turn metadata).
- `meerkat-contracts/src/wire/mob.rs:152` (`provider_params: Option<Value>` → typed wire projection) and `meerkat-contracts/src/wire/runtime.rs`.

**Propagation order.** Core type definition (`meerkat-core/src/lifecycle/run_primitive.rs`) → wire projection (`meerkat-contracts/src/wire/{mob,runtime}.rs`) → producing sites (runtime_loop construction, surface runtime_backed construction, session persistent round-trip) → consuming sites (core executor, state, factory) → SDK types.

---

### V4. Skill identity (SourceUuid, SkillName, SkillKey, CapabilityId, SourceLifecycleStatus)

**Current state.**
- `meerkat-core/src/skills/mod.rs:74-99`: `SourceUuid(Uuid)` — already typed, correct.
- `meerkat-core/src/skills/mod.rs:108-165`: `SkillName(String)` with validated slug parser.
- `meerkat-core/src/skills/mod.rs:168-173`: `SkillKey { source_uuid, skill_name }` — typed.
- `meerkat-core/src/skills/mod.rs:175-182`: `SkillRef::{Legacy(String), Structured(SkillKey)}` — **this `Legacy` variant is the boundary-compat bridge that must die**. Only `Structured` remains after wave-b.
- `meerkat-core/src/skills/mod.rs:196-204`: `SourceIdentityStatus::{Active, Disabled, Retired}` — declared but not enforced at resolve.
- `meerkat-core/src/skills/identity.rs:18` (`pub struct SourceIdentityRegistry;`): registry exists but has no `canonical_skill_key(&self, …)` nor a `resolve(&self, ref: &SkillRef) -> Result<ResolvedSkill, …>` that rejects `Disabled`/`Retired` sources.
- `meerkat-core/src/skills/identity.rs:263-291` (`parse_legacy_as_key`): the legacy string → `SkillKey` fallback lives here; wave-b deletes it, no legacy strings reach resolution.
- `meerkat-core/src/skills/mod.rs:403`: `capability_requirements: Vec<String>` stringly typed (dogma #22). No `CapabilityId` newtype exists.
- Tombstone: `meerkat-tools/src/builtin/skills/*.rs` no longer calls `canonical_key(...)` (wave-a commit `3c5f7e347` removed those sites). The non-stale `canonical_key` survivor is `meerkat-mob/src/runtime/edge_locks.rs:25,39,54` — that's an edge-lock canonicalizer, unrelated to skill identity.
- `meerkat-skills/src/source/filesystem.rs:117,311` still parses legacy ref format.
- `meerkat-core/src/agent/runner.rs:990` still lowers `SkillKey` → `SkillId(format!("{}/{}", key.source_uuid, key.skill_name))` per dogma catalog (#52).

**Target typed shape.**

```rust
// meerkat-core/src/skills/mod.rs — retire SkillRef::Legacy, retire SkillId
pub enum SkillRef { Structured(SkillKey) }   // one variant only; delete when migration done
// best: delete SkillRef entirely; pass `SkillKey` everywhere.
// DELETE `pub struct SkillId(pub String)` and all String-keyed skill id paths.

pub struct CapabilityId(String);   // validated slug; replaces raw String capability names
impl CapabilityId { pub fn parse(s: &str) -> Result<Self, SkillError>; }

// meerkat-core/src/skills/identity.rs
impl SourceIdentityRegistry {
    pub fn canonical_skill_key(&self, r: &SkillRef) -> Result<SkillKey, SkillError>;
    pub fn resolve(&self, key: &SkillKey) -> Result<ResolvedSkill<'_>, ResolveError>;
    // resolve() MUST reject Disabled/Retired sources with a typed error.
}

pub enum ResolveError {
    SourceUnknown(SourceUuid),
    SourceDisabled { source: SourceUuid, status: SourceIdentityStatus },
    SkillNotFound { source: SourceUuid, skill: SkillName },
    RemappedButTargetDisabled { from: SkillKey, to: SkillKey },
}
```

Every reference to a skill inside core/runtime/tools/mob becomes `SkillKey`. `SkillId` is deleted. `capability_requirements: Vec<CapabilityId>`.

**Dogmatic decision.** `SkillRef::Legacy` goes away entirely. All ingress surfaces (RPC `params.rs:143`, SDK, CLI) parse directly into `SkillKey` at the wire boundary. A malformed ref is a typed ingress error, not an auto-legacy-upgrade. This eliminates dogma #16, #52, #53, #14, #15, #22 in one pass.

**Consumer surface.**
- `meerkat-core/src/skills/{mod.rs, identity.rs, ...}`, `meerkat-core/src/skills_config.rs:214,267`.
- `meerkat-skills/src/{resolve.rs, engine.rs, parser.rs, source/{composite,embedded,filesystem,protocol}.rs}`.
- `meerkat-core/src/agent/runner.rs:990`, `state.rs`.
- `meerkat-tools/src/builtin/skills/{browse,load,resources,functions}.rs` (rebuild around `SourceIdentityRegistry::canonical_skill_key`).
- `meerkat-contracts/src/wire/params.rs:143` (ingress parse to `SkillKey`).
- `meerkat-rpc/src/handlers/skills.rs:49,80` (retire name-keyed inspection).

**Propagation order.** Delete `SkillRef::Legacy` + `SkillId` in core → introduce `CapabilityId` in core → registry `resolve()` method wired → skills crate parser + engine consume `CapabilityId` → filesystem/embedded/protocol sources construct typed `SkillKey` directly → ingress wire parses directly → runner/state consume.

---

### V5. Peer identity

**Current state.**
- `meerkat-core/src/comms.rs:16-53` — `PeerName(String)` with validated non-empty + no-NUL slug. Currently the routing identity.
- `meerkat-core/src/comms.rs:376-388` — `PeerDirectoryEntry { name: PeerName, peer_id: String, ... }`. `peer_id` is raw `String`.
- `meerkat-core/src/comms.rs:130, 426`, `meerkat-comms/src/trust.rs:145,154`, `meerkat-comms/src/router.rs:201`, `meerkat-comms/src/runtime/comms_runtime.rs:1650,1774`: routing + trust keying still uses `PeerName` (dogma #57).
- `meerkat-comms/src/trust.rs:21` — `InvalidPeerId` error variant already exists, suggesting intent for `PeerId` typing never landed.

**Target typed shape.**

```rust
// meerkat-core/src/comms.rs
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PeerId(Uuid);   // canonical runtime identity, opaque, never collides
impl PeerId {
    pub fn parse(s: &str) -> Result<Self, IdentityError>;
    pub fn new() -> Self;
    pub fn as_str(&self) -> String; // hyphenated uuid
}

// PeerName stays — downgraded to a display-only slug.
// Trust store and router key by PeerId.

pub struct PeerDirectoryEntry {
    pub peer_id: PeerId,          // canonical identity
    pub name: PeerName,           // display only
    pub address: PeerAddress,     // typed (new): carries transport atom + endpoint
    // ...
}

// meerkat-comms/src/trust.rs — store keyed by PeerId; duplicate PeerName is fine, duplicate PeerId is a hard error.
pub struct TrustStore { entries: BTreeMap<PeerId, TrustEntry> }

// meerkat-comms/src/router.rs — router.send(dest: PeerId, ...); resolver from PeerName -> PeerId lives in a lookup helper and returns a typed ambiguity error when a name matches >1 PeerId.
```

**Dogmatic decision.** `PeerName` as routing key is deleted; it survives as display metadata only. The dogma catalog #57 calls out that duplicate names + name-keyed routing let hidden peers shadow canonical routing — typing `PeerId` as the runtime key fixes this structurally.

**Consumer surface.**
- `meerkat-core/src/comms.rs` — add `PeerId`, retype `PeerDirectoryEntry`.
- `meerkat-comms/src/{trust,router,inbox,runtime/comms_runtime.rs}`.
- `meerkat-core/src/agent/state.rs` (peer tables).
- `meerkat-runtime/src/{comms_bridge,comms_drain,mob_adapter}.rs`.
- `meerkat-contracts/src/wire/comms.rs` (if present) + `sdks/*` peer shapes.
- `meerkat-mob/src/runtime/actor.rs` (supervisor trust edges, dogma #28).

**Propagation order.** Core `PeerId` type → wire projection → trust store + router keying → runtime+mob adapter callers → SDK.

---

### V6. Connection ref

**Current state.**
- `meerkat-core/src/connection.rs:27-50`: `ConnectionRef { realm_id: String, binding_id: String }` + `parse("<realm>:<binding>")` + `Display` that emits `"realm:binding"`. **The `parse` method is itself a dogma violation**: wave-b deletes it. No part of the runtime constructs a `ConnectionRef` from a bare string after wave-b.
- `meerkat-core/src/connection.rs:65-77`: `AuthProfile` has no `storage` field. Wave-a removed it. That means `AuthProfile.storage` being authoritative is moot — the "authoritative" path becomes: `AuthProfile` identity + realm-scoped storage configured at the realm level.
- `meerkat-core/src/connection.rs:79-126`: `CredentialSourceSpec` does *not* include `ManagedStore { profile }` — it's been deleted already. The wave-b goal from the prompt ("`CredentialSourceSpec::ManagedStore { profile }` honored") is inverted: wave-a removed the variant, so wave-b's job is to not reintroduce it and to ensure the replacement (per-binding auth-lease via `AuthMachine`) is wired.
- `meerkat-contracts/src/wire/connection.rs:20-57` — `WireConnectionRef` still has `parse("dev:default_openai")` round-trip tests (`:281-287`). Deleted as part of V6.
- `meerkat-cli/src/main.rs:3798` (`profile_id.split_once(':')`), `:1381` (doc-advertises string form), `meerkat-cli/src/mcp.rs:193` (`splitn(2, ':')`). These are the remaining string-parse holdouts.
- Dogma #36 lists `meerkat-cli/src/main.rs:1059,5315`, `meerkat-mcp-server/src/lib.rs:169,2583`, `meerkat-rest/src/lib.rs:848,924`, `sdks/web/src/types.ts:52`, `sdks/typescript/src/types.ts:575`.

**Target typed shape.** Upgrade `ConnectionRef` in core:

```rust
// meerkat-core/src/connection.rs
pub struct RealmId(String);      // newtype, slug-validated
pub struct BindingId(String);
pub struct ProfileId(String);

pub struct ConnectionRef {
    pub realm: RealmId,
    pub binding: BindingId,
    pub profile: Option<ProfileId>,  // None = "use the binding's default auth_profile"
}

// DELETE `ConnectionRef::parse(raw: &str)`.
// DELETE `impl Display` that emits "realm:binding".
// DELETE WireConnectionRef::parse.
// Replace with a wire projection that only goes through JSON struct fields.
```

The wire type (`meerkat-contracts/src/wire/connection.rs`) becomes a pure `{ realm, binding, profile? }` JSON struct — no string colon form accepted, no string colon form emitted. CLI callers that take a `--connection` flag parse `realm:binding[:profile]` at the CLI input boundary **only** (CLI input is a user interface, not a wire) and construct the typed struct; every downstream call carries the struct.

**Dogmatic decision.** Delete the string form. Keep one narrow CLI-only `parse_from_user_input()` helper in `meerkat-cli/src/cli_parse.rs` (*not* in core, *not* in contracts) that is the sole location that converts from `"a:b"` to the typed struct. Every other crate accepts only the struct.

**Consumer surface.**
- `meerkat-core/src/connection.rs`.
- `meerkat-contracts/src/wire/connection.rs` (delete `parse`, delete `Display`).
- `meerkat-cli/src/main.rs`, `mcp.rs` — one parser at the CLI boundary, everything else uses the struct.
- `meerkat-mcp-server/src/lib.rs:169,2583`.
- `meerkat-rest/src/lib.rs:848,924` — accept only the struct; 400 on string form.
- `sdks/web/src/types.ts`, `sdks/typescript/src/types.ts`.
- `meerkat/src/factory.rs` (resolver consumer).
- `meerkat-auth-core/src/resolver.rs`.

**Propagation order.** Core `ConnectionRef` → wire type (strip parse/display) → provider resolver → REST/RPC handlers → SDK regeneration (wave-d note: SDK regen is wave-d, but the wire type change in contracts must land in wave-b because wave-c will build against it).

---

### V7. Tool-error classification (AccessDenied vs NotFound)

**Current state.**
- `meerkat-core/src/error.rs:54-176`: `ToolError::AccessDenied { name }` at `:77` is already typed in core, with constructor `:143`. `NotFound { name }` at `:57` is the fallback.
- `meerkat-tools/src/dispatcher.rs:36,239,311,351`: dispatcher wrappers still return `ToolError::NotFound` for policy-denied calls.
- `meerkat-tools/src/builtin/composite.rs:404`: same pattern.
- `meerkat-core/src/agent.rs:400,439`: consumer-side mapping converts `AccessDenied` back to other classes.

**Target typed shape.** No new types needed — the typed variant exists. The job is purely propagation:

```rust
// meerkat-tools/src/dispatcher.rs
impl ToolDispatcher for PolicyGatedDispatcher {
    async fn dispatch(...) -> Result<_, ToolError> {
        if !self.policy.allows(&call.name, ...) {
            // BEFORE: Err(ToolError::NotFound { name: call.name.to_string() })
            // AFTER:
            Err(ToolError::AccessDenied { name: call.name.to_string() })
        } else { inner.dispatch(call).await }
    }
}

// meerkat-tools/src/builtin/composite.rs:404 same fix.

// Wire projection: meerkat-contracts/src/wire/result.rs needs a typed
// tool_error_class enum that serializes AccessDenied vs NotFound distinctly.
pub enum WireToolErrorClass { NotFound, AccessDenied, InvalidArguments, Timeout, Internal }
```

**Dogmatic decision.** Typed variant propagates end-to-end; no collapse at any dispatcher boundary. Access-denied is distinct in core, tools, contracts, and the wire.

**Consumer surface.**
- `meerkat-tools/src/dispatcher.rs`, `builtin/composite.rs`.
- `meerkat-core/src/agent.rs:400,439`.
- `meerkat-contracts/src/wire/result.rs` (add typed class).
- `meerkat-rpc/src/handlers/*`, `meerkat-rest/src/lib.rs` (propagate to HTTP status: 403 vs 404).
- SDK error types.

**Propagation order.** Tools dispatcher fix → contracts wire type → RPC/REST propagation → SDK type.

---

### V8. Wire contracts (runtime.rs typed rewrite)

**Current state.**
- `meerkat-contracts/src/wire/runtime.rs:61-88`: `RuntimeRetireParams`, `RuntimeResetParams`, `InputStateParams`, `InputListParams` still exist — these are the `session/retire`, `session/reset`, `session/submission`, `session/submissions` verbs. Dogma catalog says runtime-control nouns are public API surface and should be deprecated. Wave-a deleted them from TS SDK; Python SDK + docs still expose them.
- `meerkat-contracts/src/wire/runtime.rs:164-240`: `WireInputState` with 6 untyped `serde_json::Value` fields (`policy`, `terminal_outcome`, `durability`, `reconstruction_source`, `persisted_input`, `policy` at `:214`). `RuntimeAcceptResult.policy: Option<Value>` at `:214`.
- `meerkat-contracts/src/wire/runtime.rs` — no `session/accept_input` typed struct; per dogma #38 it's raw JSON.
- `meerkat-contracts/src/wire/runtime.rs` — no `RealtimeAttachmentStatuses` batch result. Grep for `realtime_attachment_statuses` returned zero. Wave-a deleted the batch on most paths; `meerkat-rpc/src/handlers/runtime.rs:3` only mentions it in a module comment. **Dogmatic decision: delete the batch entirely from schema + docs + remaining SDKs. The canonical is the per-session `session/realtime_attachment_status` singular already present at `:11-13`.**
- `meerkat-contracts/src/wire/realtime.rs:228`: `MobMember { … }` discriminator exists. CLI sends `mob_member_target` (dogma #74) — that CLI-side discriminator emission still in `meerkat-cli/src/main.rs:750,760`. Canonical wire accepts only `mob_member`.

**Target typed shape.** In `meerkat-contracts/src/wire/runtime.rs`:

```rust
/// Typed per-session accept-input params — replaces the raw JSON shape that
/// the RPC handler currently parses ad hoc.
pub struct SessionAcceptInputParams {
    pub session_id: SessionIdWire,
    pub primitive: WireRunPrimitive,      // typed projection of core RunPrimitive
    pub idempotency_key: Option<IdempotencyKey>,
    pub turn_metadata: Option<WireRuntimeTurnMetadata>,   // typed projection of V3
}

pub enum WireRunPrimitive {
    StagedInput(WireStagedRunInput),
    ImmediateAppend(WireConversationAppend),
    ImmediateContextAppend(WireConversationContextAppend),
}

/// Strong typing for the 6 Value fields in WireInputState.
pub struct WireInputState {
    pub input_id: InputIdWire,
    pub current_state: WireInputLifecycleState,
    pub policy: Option<WireInputPolicy>,                  // new typed enum
    pub terminal_outcome: Option<WireInputTerminalOutcome>, // new typed enum
    pub durability: Option<WireInputDurability>,          // new typed enum
    pub idempotency_key: Option<IdempotencyKey>,
    pub attempt_count: u32,
    pub recovery_count: u32,
    pub history: Vec<WireInputStateHistoryEntry>,
    pub reconstruction_source: Option<WireReconstructionSource>,
    pub persisted_input: Option<WirePersistedInput>,
    pub last_run_id: Option<RunIdWire>,
    pub last_boundary_sequence: Option<u64>,
    pub created_at: WireTimestamp,
    pub updated_at: WireTimestamp,
}
```

Delete `RuntimeRetireParams`, `RuntimeResetParams`, `InputStateParams`, `InputListParams`, `RuntimeRetireResult`, `RuntimeResetResult`, `InputListResult`, `InputStateResult` — wave-a removed the shell verbs; wave-b removes the wire types. (Dogmatic decision: these are dead public nouns; retaining the wire types invites re-introduction.)

Delete any `RealtimeAttachmentStatuses` batch type (`meerkat-contracts/src/wire/runtime.rs` grep already zero; docs/schemas/SDK entries removed by wave-a/cleanup).

Normalize the realtime discriminator: the wire accepts only `mob_member`. Remove `mob_member_target` emission from CLI (wave-b commits the CLI fix because it's trivial once the wire is locked).

**Consumer surface.**
- `meerkat-contracts/src/wire/runtime.rs` — retype all `Option<Value>` fields, delete retired verbs.
- `meerkat-rpc/src/handlers/runtime.rs` — consume typed params for `session/accept_input` (dogma #38).
- `meerkat-rest/src/lib.rs:1327,1658,1702,1772` — serialize/deserialize through canonical wire structs instead of `json!({...})` (dogma #73).
- `meerkat-session/src/persistent.rs` — match the new typed states into the persisted schema (migration needed for existing durable rows — store a version byte and a `from_v0` migration).
- `docs/api/rpc.mdx`, `artifacts/schemas/rpc-methods.json` — wave-d regeneration; contracts crate tests in wave-b must enforce schema freshness via existing `verify-schema-freshness` gate.
- SDK generated types: wave-d regeneration.

**Propagation order.** Wire type rewrite (contracts) → RPC + REST handlers accept typed structs → session persistent migration → codegen artifact freshness gates.

---

## Section 2 — Cross-cutting CI / governance

Rows #42, #43, #44, #45, #62, #63 require CI changes. Makefile already wires `rmat-audit` into `make ci` (`Makefile:206`); the GitHub `gate` job does *not* require it (`.github/workflows/ci.yml:387-419` — `rmat-audit` absent from the needs list).

### 2.1 GitHub CI gates to add

- **`rmat-audit` job**: `cargo run -p xtask -- rmat-audit --strict` + `cargo run -p xtask -- ownership-ledger --check-drift`. Add as a required entry in `gate.needs`.
- **`seam-inventory` job**: `cargo run -p xtask -- seam-inventory --strict` (new — see 2.2). Required.
- **`machine-codegen-drift` job**: `cargo run -p xtask -- machine-check-drift` (already in Makefile, not in workflow). Required.
- **`audit-generated-headers` job**: `cargo run -p xtask -- audit-generated-headers`. Required.
- **`wasm-contract-tests` job**: run `meerkat-web-runtime/tests/browser_contract.rs` as `--ignored` off (required); currently `.github/workflows/ci.yml:349-375` only does `cargo check`/`clippy` for wasm32. Add `cargo test -p meerkat-web-runtime --target wasm32-unknown-unknown --no-run` plus a wasm-bindgen-test runner invocation. Required.

### 2.2 xtask gates that must get teeth

- `xtask/src/seam_inventory.rs:161-198` — the `_ => heuristic fallback` arm must become a hard error (`--strict` mode fails on any `was_explicit == false`). Add entries to `known_seam_classifications()` for every Local/External effect currently hitting the fallback.
- `xtask/src/seam_inventory.rs:188-197` (the `EffectDisposition::Routed` arm "handled by composition routes, not seam inventory") — routed effects **must** be classified by a new routed-effect inventory that verifies each routed effect has a typed `RouteId` that resolves to a typed consumer input in the composition schema. Add `verify_routed_effect_realization()` that consumes the V2 typed composition dispatcher tables.
- `xtask/src/rmat_audit.rs:443` ("routed effect has no realization rule") — currently treated as rule-only. Upgrade to **semantic** coverage: for each routed effect, assert the composition dispatcher table maps it to a concrete `InputVariantId` and that the consumer machine's schema accepts that variant in the producer's terminal phases. Semantic coverage = "effect emitted AND consumer input applied AND consumer transition exists from producer-visible phases".
- `xtask/src/rmat_policy.rs:39-43` — add `AuthMachine` to the hard-authority module list (dogma #43).
- `xtask/src/ownership_ledger.rs:30-34` — add `AuthMachine`/auth-lease boundary family (dogma #44). Update `docs/architecture/finite-ownership-ledger.md:16,18,24,36` accordingly.
- `xtask/src/rmat_policy.rs:65,79,161` — tighten routed-seam RMAT to reject rule-existence-only satisfaction (dogma #41).

### 2.3 Tests to turn from ignored to required

- `meerkat-web-runtime/tests/browser_contract.rs:115,279` — remove `#[ignore]`, run under `wasm-bindgen-test` in CI.
- `meerkat-web-runtime/tests/release_targets.rs:11` — mark required.

### 2.4 Exclusions to remove

- Remove any `#[cfg(not(wasm))]` gates on portability tests that are compile-only.
- Remove `#[allow(dead_code)]` on `MobMachineCommand::{EnsureMember, Reconcile, ListMembersMatching}` in `meerkat-mob/src/mob_machine.rs:44,50,56` — these either get wired through the composition dispatcher (V2) or deleted.

---

## Section 3 — Task breakdown (10 worker agents)

Strict file allowlist per agent; each agent owns one vertical or a clean slice.

### Task B-1: Machine ID newtypes (V1 foundation)
- **Owns:** `meerkat-machine-schema/src/types.rs` (new `identity` submodule), `meerkat-machine-schema/src/lib.rs`.
- **Deliverable:** the newtypes in V1 target shape above. `impl PartialOrd, Ord, Hash, Serialize, Deserialize, Display`. `parse` validates non-empty ASCII-identifier-compatible slug.
- **Tests:** `meerkat-machine-schema/tests/identity_roundtrip.rs` — typed round-trip (JSON serialize → deserialize → equality), rejection of empty / invalid slugs, hash stability.

### Task B-2: Machine schema + composition retyping (V1)
- **Depends on:** B-1.
- **Owns:** `meerkat-machine-schema/src/machine.rs`, `meerkat-machine-schema/src/composition.rs`, `meerkat-machine-schema/src/catalog/**`.
- **Deliverable:** every `String` field in V1's "Current state" section replaced with the matching newtype. `NamedTypeBinding` struct on every named type declaration in the DSL.
- **Tests:** `meerkat-machine-schema/tests/catalog_typed_round_trip.rs` — build every catalog composition, assert all identities are typed newtypes, snapshot the kernel shape via `insta`.

### Task B-3: Kernel runtime retyping (V1)
- **Depends on:** B-2.
- **Owns:** `meerkat-machine-kernels/src/runtime.rs`, `meerkat-machine-kernels/src/generated/**` (regenerated).
- **Deliverable:** `KernelState`, `KernelInput`, `KernelSignal`, `KernelEffect`, `TransitionOutcome`, `TransitionRefusal` use V1 newtypes.
- **Tests:** `meerkat-machine-kernels/tests/kernel_typed_round_trip.rs` — apply input/produce effect, assert typed identities, serde round-trip with typed-ID JSON shape.

### Task B-4: Codegen named-type binding + route emitter (V1 + V2)
- **Depends on:** B-1, B-2, B-3.
- **Owns:** `meerkat-machine-codegen/src/render.rs`, `meerkat-machine-codegen/src/artifacts.rs`, `meerkat-machine-codegen/src/lib.rs`.
- **Deliverable:** Delete `render_named_type_alias_target`. Consume `NamedTypeBinding` from schema. Emit typed per-composition modules that expose `EffectPayload<E>` + `route_to_input(&E) -> Option<TypedRoutedInput>`.
- **Tests:** `meerkat-machine-codegen/tests/named_type_binding_respected.rs` — a custom named type with `RustTypeAtom::TypePath("MyType")` lowers to `MyType`, not `String`. `meerkat-machine-codegen/tests/routed_effect_module_shape.rs` — generated `meerkat_mob_seam` module has expected types + `route_to_input` fn.

### Task B-5: Composition dispatcher (V2)
- **Depends on:** B-4.
- **Owns:** `meerkat-runtime/src/composition/` (new directory), `meerkat-runtime/src/lib.rs` (add `pub mod composition`).
- **Deliverable:** `CompositionDispatcher` trait + default impl that consumes the codegen-emitted module. Wire it into `meerkat-runtime/src/meerkat_machine/dispatch_*.rs` as *the* routed-effect path.
- **Tests:** `meerkat-runtime/tests/composition_dispatch_is_the_path.rs` — produce a `MobMachine` routed effect, assert `CompositionDispatcher::dispatch` was called, assert the target `MeerkatMachine` input was applied typed. No ad-hoc helper path works.

### Task B-6: Runtime turn metadata (V3)
- **Depends on:** B-1 (optional; `ModelId` newtype is independent but conceptually the same pattern).
- **Owns:** `meerkat-core/src/lifecycle/run_primitive.rs`, `meerkat-runtime/src/runtime_loop.rs`, `meerkat-contracts/src/wire/runtime.rs` (typed projection part only), `meerkat-contracts/src/wire/mob.rs:152`.
- **Deliverable:** V3 target shape. Single `RuntimeTurnMetadata::for_input` constructor. Typed `provider_params` (delete `serde_json::Value`). Typed `keep_alive` + `connection_ref` fields.
- **Tests:** `meerkat-core/tests/runtime_turn_metadata_typed_round_trip.rs` — round-trip every field through JSON, assert scalar-conflict in batch merge refuses. `meerkat-runtime/tests/turn_metadata_single_construction_site.rs` — compile-time assertion that `RuntimeTurnMetadata::default()` is not called from anywhere in `meerkat-runtime/src/` outside `runtime_loop::for_input`.

### Task B-7: Skill identity finalize (V4)
- **Depends on:** none (SkillKey exists).
- **Owns:** `meerkat-core/src/skills/mod.rs`, `meerkat-core/src/skills/identity.rs`, `meerkat-skills/src/**`, `meerkat-tools/src/builtin/skills/**`, `meerkat-core/src/agent/runner.rs` (runner.rs:990 lowering), `meerkat-contracts/src/wire/params.rs:143`.
- **Deliverable:** Delete `SkillId` + `SkillRef::Legacy`. Introduce `CapabilityId`. `SourceIdentityRegistry::canonical_skill_key` + `resolve` with lifecycle-status rejection. Tool builtins rebuilt around the typed path.
- **Tests:** `meerkat-core/tests/source_identity_resolve_enforces_lifecycle.rs` — resolve a key against a `Disabled` source returns `ResolveError::SourceDisabled`. `meerkat-tools/tests/builtin_skill_canonical_key.rs` — every builtin skill tool retrieves `SkillKey` via registry, no parsing of slash-strings. `meerkat-contracts/tests/skill_ref_wire_no_legacy.rs` — round-trip wire `SkillRef` with only `Structured` form.

### Task B-8: Peer identity (V5)
- **Depends on:** none.
- **Owns:** `meerkat-core/src/comms.rs` (PeerId type), `meerkat-comms/src/{trust,router,inbox,runtime/comms_runtime.rs}`, `meerkat-contracts/src/wire/comms.rs` (if exists; else wire/session.rs peer fields).
- **Deliverable:** `PeerId(Uuid)` newtype. Trust store keyed by `PeerId`. Router `send(dest: PeerId)`. Name → id resolver returns typed ambiguity error.
- **Tests:** `meerkat-comms/tests/trust_duplicate_name_ok_duplicate_id_rejected.rs`. `meerkat-comms/tests/router_ambiguous_name_typed_error.rs`.

### Task B-9: ConnectionRef + tool-error + wire typed (V6 + V7 + V8)
- **Depends on:** V3 is in-flight but wire projection can proceed in parallel.
- **Owns:** `meerkat-core/src/connection.rs` (add `RealmId`, `BindingId`, `ProfileId`, retype `ConnectionRef`, delete `parse` + `Display`), `meerkat-contracts/src/wire/connection.rs` (delete parse/display), `meerkat-contracts/src/wire/runtime.rs` (typed rewrite — SessionAcceptInputParams, typed WireInputState fields, delete retired verbs/results), `meerkat-contracts/src/wire/result.rs` (add `WireToolErrorClass`), `meerkat-tools/src/dispatcher.rs`, `meerkat-tools/src/builtin/composite.rs`, `meerkat-rpc/src/handlers/runtime.rs`, `meerkat-rest/src/lib.rs` (runtime handlers).
- **Deliverable:** V6, V7, V8 target shapes.
- **Tests:** `meerkat-core/tests/connection_ref_no_string_form.rs` — compile-fails a `ConnectionRef::parse(...)` call. `meerkat-contracts/tests/session_accept_input_typed_round_trip.rs`. `meerkat-tools/tests/dispatcher_access_denied_distinct.rs` — blocked tool returns `AccessDenied`, missing tool returns `NotFound`. `meerkat-rest/tests/runtime_no_json_macro.rs` — REST runtime handlers compile without `json!({...})`.

### Task B-10: xtask governance lanes (Section 2)
- **Depends on:** B-3, B-5 (routed-effect inventory needs typed composition dispatch).
- **Owns:** `xtask/src/rmat_audit.rs`, `xtask/src/rmat_policy.rs`, `xtask/src/seam_inventory.rs`, `xtask/src/ownership_ledger.rs`, `Makefile` (already has `rmat-audit`), `.github/workflows/ci.yml`, `docs/architecture/finite-ownership-ledger.md`.
- **Deliverable:** `AuthMachine` added to hard-authority list, routed effects semantically audited, seam inventory's heuristic fallback errors in `--strict`, all new lanes added to GitHub `gate.needs`, WASM contract tests un-ignored.
- **Tests:** `xtask` unit tests in `rmat_audit`/`seam_inventory` that fail when an unclassified effect hits the fallback. A CI dry-run `make ci` passing locally is the acceptance criterion.

### Dependency graph

```
B-1 (ID newtypes)
  └─ B-2 (schema/composition retype)
        └─ B-3 (kernel retype)
              └─ B-4 (codegen)
                    └─ B-5 (composition dispatcher)
                          └─ B-10 (xtask governance, routed-effect audit)

B-6 (turn metadata)     — parallel to V1 chain; only touches core/runtime
B-7 (skill identity)    — parallel to V1 chain; only touches core/skills/tools
B-8 (peer identity)     — parallel to V1 chain; only touches core/comms
B-9 (wire typed + conn + tool-err) — parallel; depends on B-6 for wire turn-metadata projection only
```

---

## Section 4 — Foundation ordering (waves within wave-b)

**b.1 — Identity substrate (blocks everything):** B-1.

**b.2 — Typed schema + kernel (parallel trio):** B-2, B-3, B-4 can land in a tight chain (B-2 → B-3 → B-4); B-6, B-7, B-8 can run concurrently with b.2.

**b.3 — Typed runtime surface + governance:** B-5 (composition dispatcher), B-9 (wire + connection + tool-error). B-10 (xtask) lands last because its routed-effect semantic audit requires B-5 to have wired the dispatcher.

Why this ordering is not pure parallelism: B-4 (codegen) consumes B-2's `NamedTypeBinding`; B-5 consumes B-4's emitted modules; B-10's routed-effect semantic audit needs B-5's `CompositionDispatcher` table in place to verify producer→consumer realization.

---

## Section 5 — Explicit non-goals

- **No shell rebuild (wave c).** Wave-b does not rebuild `meerkat-runtime/src/meerkat_machine/dispatch_*.rs` beyond what B-5 requires to wire the composition dispatcher — dispatching *through* the new typed seam is wave-b; rewriting dispatcher internals is wave-c.
- **No surface rebuild (wave d).** RPC, REST, MCP, CLI handlers change only where the wire contract type changes (B-9). No new surfaces, no handler refactors, no new endpoints.
- **No doc regeneration (wave d).** `docs/api/rpc.mdx`, `docs/api/rest.mdx`, `artifacts/schemas/rpc-methods.json`, SDK types are *not* regenerated here. Wave-b only ensures the `verify-schema-freshness` gate fails — wave-d runs the regen.
- **No reintroduction of deleted shadow state.** The `composition_dispatch.rs`, `recompute_mob_peer_overlay*.rs`, `mob_member_lifecycle_authority.rs`, `CommsTrustReconciler` files *stay deleted*. B-5's `meerkat-runtime/src/composition/` is a new typed tree with a different responsibility (dispatch-as-execution-path, not shadow state).
- **No `Option<Arc<dyn Trait>>` optionalization.** The `CompositionDispatcher` handle on `MeerkatMachine` is not `Option<Arc<dyn CompositionDispatcher>>`. Either the machine has a dispatcher (closed-world composition) or it does not (single-machine test harness) — the type distinguishes these at compile time via two constructors.
- **No `SkillRef::Legacy` preservation.** The variant is deleted in B-7. There is no "legacy-mode" serde feature flag.
- **No `serde_json::Value` in seams.** `WireInputState`'s six `Value` fields all become typed enums in B-9. If a wire field cannot be typed without blocking wave-b, it moves to `StructuredProviderExtension` — a typed opaque-bag newtype that is explicitly marked non-semantic and gated behind a dogma-allowed exemption in `xtask/src/rmat_policy.rs`.
- **No turn-limit parity "test later" deferral.** If B-6 surfaces the dogma #4 divergence (standalone vs runtime-backed), a typed assertion test lands in wave-b, not wave-c.

---

## Section 6 — Risk register

1. **Risk: V1 retyping explodes through `meerkat-machine-schema/src/catalog/**` with stale DSL copies.** The wave-a report flags a duplicate DSL in `meerkat-machine-schema/src/catalog/dsl/mob_machine.rs` vs `meerkat-mob/src/machines/mob_machine.rs`. A typed refactor that only touches one copy reintroduces the fork.
   - **Catching assertion:** `xtask/src/machines.rs` grows a `check_dsl_parity` lane that hashes the semantic structure of both DSL sources and fails CI if they diverge. A single-source rule is enforced: the mob crate's copy is deleted; only the schema catalog copy survives.

2. **Risk: B-6 (turn metadata) breaks persisted session rows.** Adding typed `keep_alive`, `connection_ref`, `provider_params` fields changes the persisted JSON shape in `meerkat-session/src/persistent.rs:1251,1448`.
   - **Catching assertion:** `meerkat-session/tests/persistent_round_trip_legacy.rs` loads a fixture row in pre-wave-b shape and asserts the typed deserialize succeeds (v0 → v1 migration). Fixture lives in `test-fixtures/session/pre_wave_b/*.json`.

3. **Risk: B-5 composition dispatcher becomes "optional fast path" instead of *the* path.** Reviewer pressure to "keep a fallback helper for tests" reintroduces dogma #75/#76/#77.
   - **Catching assertion:** `meerkat-runtime/tests/composition_dispatch_is_the_path.rs` uses `grep_rs` (or a build.rs script) to fail the build if *any* file under `meerkat-runtime/src/meerkat_machine/` or `meerkat-mob/src/runtime/` applies a routed effect's target input *not* via `CompositionDispatcher::dispatch`. The check runs in `rmat-audit`.

4. **Risk: `PeerName` continues to creep into routing after V5.** `PeerName` remaining as a display type invites callers to keep using it as a key.
   - **Catching assertion:** `meerkat-comms/tests/peer_name_not_a_routing_key.rs` asserts `TrustStore` has no `lookup_by_name` method and `Router::send` signature takes `PeerId`. An xtask lane greps `meerkat-comms/src`, `meerkat-core/src/comms.rs`, and `meerkat-runtime/src/comms_*.rs` for `HashMap<PeerName,` / `BTreeMap<PeerName,` as routing tables and fails.

5. **Risk: `ConnectionRef::parse` deletion leaves CLI callers silently migrating to ad-hoc splitters.** `meerkat-cli/src/main.rs:3798` and `mcp.rs:193` already use `split_once(':')`/`splitn(2, ':')` inline.
   - **Catching assertion:** `meerkat-cli/tests/connection_ref_single_parser.rs` asserts there is exactly one `split_once(':')` call site involving a connection-like string across `meerkat-cli/src/**` (the single `cli_parse::parse_connection_ref_user_input` function).

6. **Risk: Wave-b ships with typed foundations but the `rmat-audit` job remains non-blocking in GitHub because `gate.needs` is not updated.** Makefile already requires it (`Makefile:206`), but the GitHub gate doesn't (`.github/workflows/ci.yml:387-419`). This means wave-b's governance can silently regress post-merge.
   - **Catching assertion:** `xtask/tests/ci_gate_requires_rmat.rs` reads `.github/workflows/ci.yml`, parses the YAML, and asserts `jobs.gate.needs` contains `rmat-audit`, `seam-inventory`, `audit-generated-headers`, `machine-codegen-drift`, `wasm-contract-tests`. Fails the wave-b merge if the gate list doesn't include them.
