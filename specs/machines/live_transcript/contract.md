# LiveTranscriptMachine

_Generated from the Rust machine catalog. Do not edit by hand._

- Version: `7`
- Rust owner: `self` / `catalog::dsl::live_transcript`

## State
- Phase enum: `Ready`
- `ingress_open`: `Bool`
- `ingress_generation`: `u64`
- `channels`: `Set<String>`
- `accepting_channels`: `Set<String>`
- `source_accepting_channels`: `Set<String>`
- `activation_sequences`: `Map<String, u64>`
- `receive_ordinals`: `Map<String, u64>`
- `durable_watermarks`: `Map<String, u64>`
- `reservation_frontiers`: `Map<String, u64>`
- `gap_channels`: `Map<u64, String>`
- `control_credit_records`: `Map<String, u64>`
- `control_credit_bytes`: `Map<String, u64>`
- `control_maximum_record_charge`: `Map<String, u64>`
- `control_spent_records`: `Map<String, u64>`
- `control_spent_bytes`: `Map<String, u64>`
- `voice_channels`: `Set<String>`
- `voice_observation_open`: `Set<String>`
- `voice_usage_observed`: `Set<String>`
- `voice_final_observed`: `Set<String>`
- `voice_seconds_bits`: `Map<String, u64>`
- `voice_usage_disputes`: `Map<String, LiveVoiceUsageDispute>`
- `voice_usage_digests`: `Map<String, String>`
- `voice_usage_sequences`: `Map<String, u64>`
- `provider_start_digests`: `Map<String, String>`
- `provider_control_sequences`: `Map<String, u64>`
- `provider_control_channels`: `Map<String, String>`

## Inputs
- `ActivateChannel`(channel: String, sequence: u64, ingress_generation: u64, credit_records: u64, credit_bytes: u64, maximum_record_charge: u64, voice_accounting: Bool)
- `AppendObservation`(channel: String, sequence: u64, receive_ordinal: u64, ingress_generation: u64)
- `RecordKnownGap`(channel: String, sequence: u64, after_received: u64, through_received: u64, ingress_generation: u64, record_bytes: u64)
- `RecoverUnknownTail`(channel: String, sequence: u64, record_bytes: u64)
- `CloseChannel`(channel: String, sequence: u64, record_bytes: u64)
- `CloseIngress`(ingress_generation: u64)
- `CloseCurrentIngress`
- `ObserveSourceIngress`(channel: String)
- `FenceChannelSources`(channel: String)
- `ReservePrefix`(channel: String, after: u64, through: u64, received_through: u64, ingress_generation: u64)
- `SelectExplicitRange`(channel: String, after: u64, through: u64, ingress_generation: u64)
- `FenceKnownReceiveTail`(channel: String, sequence: u64, after_received: u64, through_received: u64, record_bytes: u64)
- `ObserveVoiceUsage`(channel: String, sequence: u64, kind: LiveVoiceUsageKind, seconds_bits: u64, digest: String, record_bytes: u64)
- `ObserveProviderControl`(channel: String, sequence: u64, kind: LiveProviderControlKind, digest: String, record_bytes: u64)

## Signals

## Effects
- `ChannelActivated`(channel: String, sequence: u64)
- `ObservationAccepted`(channel: String, sequence: u64, receive_ordinal: u64)
- `KnownGapAccepted`(channel: String, sequence: u64, after_received: u64, through_received: u64)
- `UnknownTailFenced`(channel: String, sequence: u64)
- `ChannelIngressClosed`(channel: String, sequence: u64)
- `IngressClosed`(ingress_generation: u64)
- `AwaitingObservationDurability`(channel: String)
- `RangeSelected`(channel: String, after: u64, through: u64, discontinuous: Bool)
- `KnownTailFenced`(channel: String, sequence: u64, after_received: u64, through_received: u64)
- `VoiceUsageRecorded`(channel: String, sequence: u64)
- `VoiceUsageUnchanged`(channel: String, sequence: u64)
- `SourceIngressObserved`(channel: String, ingress_open: Bool)
- `ChannelSourcesFenced`(channel: String)
- `ProviderControlRecorded`(channel: String, digest: String, sequence: u64)
- `ProviderControlUnchanged`(channel: String, digest: String, sequence: u64)
- `ProviderControlRefused`(channel: String, reason: LiveProviderControlRefusal)

## Invariants
- `channel_coverage_is_complete`
- `control_capacity_is_retained`
- `voice_accounting_remains_channel_scoped`
- `provider_controls_retain_exact_receipts`

## Transitions
### `ObserveRepeatedProviderControl`
- From: `Ready`
- On: `ObserveProviderControl`(channel, sequence, kind, digest, record_bytes)
- Guards:
  - ``
- Emits: `ProviderControlUnchanged`
- To: `Ready`

### `RefuseConflictingProviderStart`
- From: `Ready`
- On: `ObserveProviderControl`(channel, sequence, kind, digest, record_bytes)
- Guards:
  - ``
- Emits: `ProviderControlRefused`
- To: `Ready`

### `RefuseLateProviderControl`
- From: `Ready`
- On: `ObserveProviderControl`(channel, sequence, kind, digest, record_bytes)
- Guards:
  - ``
- Emits: `ProviderControlRefused`
- To: `Ready`

### `RecordProviderControl`
- From: `Ready`
- On: `ObserveProviderControl`(channel, sequence, kind, digest, record_bytes)
- Guards:
  - ``
- Emits: `ProviderControlRecorded`
- To: `Ready`

### `RefuseProviderControlCapacity`
- From: `Ready`
- On: `ObserveProviderControl`(channel, sequence, kind, digest, record_bytes)
- Guards:
  - ``
- Emits: `ProviderControlRefused`
- To: `Ready`

### `ObserveExactSourceIngress`
- From: `Ready`
- On: `ObserveSourceIngress`(channel)
- Guards:
  - ``
- Emits: `SourceIngressObserved`
- To: `Ready`

### `FenceExactChannelSources`
- From: `Ready`
- On: `FenceChannelSources`(channel)
- Guards:
  - ``
- Emits: `ChannelSourcesFenced`
- To: `Ready`

### `ActivateFreshChannel`
- From: `Ready`
- On: `ActivateChannel`(channel, sequence, ingress_generation, credit_records, credit_bytes, maximum_record_charge, voice_accounting)
- Guards:
  - ``
- Emits: `ChannelActivated`
- To: `Ready`

### `ObserveRepeatedVoiceUsage`
- From: `Ready`
- On: `ObserveVoiceUsage`(channel, sequence, kind, seconds_bits, digest, record_bytes)
- Guards:
  - ``
- Emits: `VoiceUsageUnchanged`
- To: `Ready`

### `ReconcileVoiceUsage`
- From: `Ready`
- On: `ObserveVoiceUsage`(channel, sequence, kind, seconds_bits, digest, record_bytes)
- Guards:
  - ``
- Emits: `VoiceUsageRecorded`
- To: `Ready`

### `AcceptNextObservation`
- From: `Ready`
- On: `AppendObservation`(channel, sequence, receive_ordinal, ingress_generation)
- Guards:
  - ``
- Emits: `ObservationAccepted`
- To: `Ready`

### `AcceptKnownReceiveGap`
- From: `Ready`
- On: `RecordKnownGap`(channel, sequence, after_received, through_received, ingress_generation, record_bytes)
- Guards:
  - ``
- Emits: `KnownGapAccepted`
- To: `Ready`

### `FenceUnknownCrashTail`
- From: `Ready`
- On: `RecoverUnknownTail`(channel, sequence, record_bytes)
- Guards:
  - ``
- Emits: `UnknownTailFenced`
- To: `Ready`

### `FenceKnownUncommittedTail`
- From: `Ready`
- On: `FenceKnownReceiveTail`(channel, sequence, after_received, through_received, record_bytes)
- Guards:
  - ``
- Emits: `KnownTailFenced`
- To: `Ready`

### `CloseExactChannelIngress`
- From: `Ready`
- On: `CloseChannel`(channel, sequence, record_bytes)
- Guards:
  - ``
- Emits: `ChannelIngressClosed`
- To: `Ready`

### `FenceSessionIngress`
- From: `Ready`
- On: `CloseIngress`(ingress_generation)
- Guards:
  - ``
- Emits: `IngressClosed`
- To: `Ready`

### `FenceCurrentSessionIngress`
- From: `Ready`
- On: `CloseCurrentIngress`()
- Guards:
  - ``
- Emits: `IngressClosed`
- To: `Ready`

### `ObserveClosedSessionIngress`
- From: `Ready`
- On: `CloseCurrentIngress`()
- Guards:
  - ``
- Emits: `IngressClosed`
- To: `Ready`

### `AwaitUncommittedObservations`
- From: `Ready`
- On: `ReservePrefix`(channel, after, through, received_through, ingress_generation)
- Guards:
  - ``
- Emits: `AwaitingObservationDurability`
- To: `Ready`

### `ReserveDurablePrefix`
- From: `Ready`
- On: `ReservePrefix`(channel, after, through, received_through, ingress_generation)
- Guards:
  - ``
- Emits: `RangeSelected`
- To: `Ready`

### `SelectDurableExplicitRange`
- From: `Ready`
- On: `SelectExplicitRange`(channel, after, through, ingress_generation)
- Guards:
  - ``
- Emits: `RangeSelected`
- To: `Ready`

## Coverage
### Code Anchors
- `live_transcript_catalog_bridge` (machine `LiveTranscriptMachine`): `meerkat-runtime/src/live_ledger/transcript_authority/dsl.rs` — catalog-derived observation rules only; this anchor does not claim native persistence or source reservation realization

### Scenarios
- `generated_transcript_append_requires_contiguous_receive_identity` — generated observation guards preserve exact channel, receive ordinal, durable sequence, and ingress generation
- `generated_prefix_reservation_spends_frontier_and_preserves_gap_facts` — generated range selection waits for durability and preserves source-frontier and channel-specific gap facts; not a native source commit proof
- `generated_transcript_control_credit_always_preserves_final_fence` — generated control spending leaves one final fence despite gap records and rejects observation append after ingress closure
- `generated_crash_fence_never_advances_received_ordinal` — generated unknown-tail transition changes no received count and requires a new channel before range selection
- `native_transcript_loss_closes_known_tail_with_last_credit` — native Memory/WholeBlob/HeadCanonical writer retains exact failed receive bounds and uses the last reserved slot to close the known tail without inventing crash uncertainty
- `native_transcript_cold_recovery_cannot_guess_uncommitted_receive_count` — full SQLite store close and reopen with equal durable Live states but zero versus seven lost local receives produces the same unknown-extent recovery snapshot
- `native_voice_accounting_retains_control_credit_through_observation_closure` — native Memory/WholeBlob/HeadCanonical tests reconcile cumulative seconds, duplicate receipts and disputes; provider observation closure, not text ingress closure, releases control credit. Native quota tests cover maximum escaped channel and finite duration at zero free capacity. This does not claim full public host or formal qualification.
- `source_drain_fence_keeps_observation_ingress_without_admitting_new_work` — native source-drain tests fence source selection and initial admission at one exact Live head while still persisting TEXT and preserving already-admitted scope. This is the native fence, not observation-pump handoff qualification.
- `native_provider_controls_replay_exact_facts_without_ordinary_mutation` — native control tests bind start and diagnostic receipts to exact session/channel/content, preserve duplicate A/B/A across appends and reopen, refuse conflicting start and new closed/capacity-exhausted controls, and measure snapshot growth at zero free quota. Shared-pump native tests route only committed controls without ordinary mutation. Not full readiness, function, or formal qualification.
