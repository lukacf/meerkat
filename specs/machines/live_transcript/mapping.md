# LiveTranscriptMachine Mapping Note

<!-- GENERATED_COVERAGE_START -->
## Generated Coverage
This section is generated from the Rust machine catalog. Do not edit it by hand.

### Machine
- `LiveTranscriptMachine`

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

### Transitions
- `ObserveRepeatedProviderControl`
  - anchors: (unclaimed)
  - scenarios: `native_provider_controls_replay_exact_facts_without_ordinary_mutation`
- `RefuseConflictingProviderStart`
  - anchors: (unclaimed)
  - scenarios: `native_provider_controls_replay_exact_facts_without_ordinary_mutation`
- `RefuseLateProviderControl`
  - anchors: (unclaimed)
  - scenarios: `native_provider_controls_replay_exact_facts_without_ordinary_mutation`
- `RecordProviderControl`
  - anchors: (unclaimed)
  - scenarios: `native_provider_controls_replay_exact_facts_without_ordinary_mutation`
- `RefuseProviderControlCapacity`
  - anchors: (unclaimed)
  - scenarios: `native_provider_controls_replay_exact_facts_without_ordinary_mutation`
- `ObserveExactSourceIngress`
  - anchors: (unclaimed)
  - scenarios: `source_drain_fence_keeps_observation_ingress_without_admitting_new_work`
- `FenceExactChannelSources`
  - anchors: (unclaimed)
  - scenarios: `source_drain_fence_keeps_observation_ingress_without_admitting_new_work`
- `ActivateFreshChannel`
  - anchors: (unclaimed)
  - scenarios: `generated_transcript_append_requires_contiguous_receive_identity`
- `ObserveRepeatedVoiceUsage`
  - anchors: (unclaimed)
  - scenarios: `native_voice_accounting_retains_control_credit_through_observation_closure`
- `ReconcileVoiceUsage`
  - anchors: (unclaimed)
  - scenarios: `native_voice_accounting_retains_control_credit_through_observation_closure`
- `AcceptNextObservation`
  - anchors: (unclaimed)
  - scenarios: `generated_transcript_append_requires_contiguous_receive_identity`
- `AcceptKnownReceiveGap`
  - anchors: (unclaimed)
  - scenarios: `generated_prefix_reservation_spends_frontier_and_preserves_gap_facts`
- `FenceUnknownCrashTail`
  - anchors: (unclaimed)
  - scenarios: `generated_crash_fence_never_advances_received_ordinal`, `native_transcript_cold_recovery_cannot_guess_uncommitted_receive_count`
- `FenceKnownUncommittedTail`
  - anchors: (unclaimed)
  - scenarios: `native_transcript_loss_closes_known_tail_with_last_credit`
- `CloseExactChannelIngress`
  - anchors: (unclaimed)
  - scenarios: `generated_transcript_control_credit_always_preserves_final_fence`
- `FenceSessionIngress`
  - anchors: (unclaimed)
  - scenarios: `generated_transcript_control_credit_always_preserves_final_fence`
- `FenceCurrentSessionIngress`
  - anchors: (unclaimed)
  - scenarios: (unclaimed)
- `ObserveClosedSessionIngress`
  - anchors: (unclaimed)
  - scenarios: (unclaimed)
- `AwaitUncommittedObservations`
  - anchors: (unclaimed)
  - scenarios: `generated_prefix_reservation_spends_frontier_and_preserves_gap_facts`
- `ReserveDurablePrefix`
  - anchors: (unclaimed)
  - scenarios: `generated_prefix_reservation_spends_frontier_and_preserves_gap_facts`
- `SelectDurableExplicitRange`
  - anchors: (unclaimed)
  - scenarios: `generated_prefix_reservation_spends_frontier_and_preserves_gap_facts`

### Effects
- `ChannelActivated`
  - anchors: (unclaimed)
  - scenarios: (unclaimed)
- `ObservationAccepted`
  - anchors: (unclaimed)
  - scenarios: (unclaimed)
- `KnownGapAccepted`
  - anchors: (unclaimed)
  - scenarios: (unclaimed)
- `UnknownTailFenced`
  - anchors: (unclaimed)
  - scenarios: (unclaimed)
- `ChannelIngressClosed`
  - anchors: (unclaimed)
  - scenarios: (unclaimed)
- `IngressClosed`
  - anchors: (unclaimed)
  - scenarios: (unclaimed)
- `AwaitingObservationDurability`
  - anchors: (unclaimed)
  - scenarios: (unclaimed)
- `RangeSelected`
  - anchors: (unclaimed)
  - scenarios: (unclaimed)
- `KnownTailFenced`
  - anchors: (unclaimed)
  - scenarios: (unclaimed)
- `VoiceUsageRecorded`
  - anchors: (unclaimed)
  - scenarios: `native_voice_accounting_retains_control_credit_through_observation_closure`
- `VoiceUsageUnchanged`
  - anchors: (unclaimed)
  - scenarios: `native_voice_accounting_retains_control_credit_through_observation_closure`
- `SourceIngressObserved`
  - anchors: (unclaimed)
  - scenarios: `source_drain_fence_keeps_observation_ingress_without_admitting_new_work`
- `ChannelSourcesFenced`
  - anchors: (unclaimed)
  - scenarios: `source_drain_fence_keeps_observation_ingress_without_admitting_new_work`
- `ProviderControlRecorded`
  - anchors: (unclaimed)
  - scenarios: `native_provider_controls_replay_exact_facts_without_ordinary_mutation`
- `ProviderControlUnchanged`
  - anchors: (unclaimed)
  - scenarios: `native_provider_controls_replay_exact_facts_without_ordinary_mutation`
- `ProviderControlRefused`
  - anchors: (unclaimed)
  - scenarios: `native_provider_controls_replay_exact_facts_without_ordinary_mutation`

### Invariants
- `channel_coverage_is_complete`
  - anchors: (unclaimed)
  - scenarios: (unclaimed)
- `control_capacity_is_retained`
  - anchors: (unclaimed)
  - scenarios: (unclaimed)
- `voice_accounting_remains_channel_scoped`
  - anchors: (unclaimed)
  - scenarios: `native_voice_accounting_retains_control_credit_through_observation_closure`
- `provider_controls_retain_exact_receipts`
  - anchors: (unclaimed)
  - scenarios: `native_provider_controls_replay_exact_facts_without_ordinary_mutation`


<!-- GENERATED_COVERAGE_END -->
