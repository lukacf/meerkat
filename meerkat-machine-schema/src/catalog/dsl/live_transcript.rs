//! Independent observation coverage and source-range authority.
//! Payload bytes and source deduplication remain on their existing owners.

use super::OptionValueExt;

#[macro_export]
macro_rules! live_transcript_catalog_machine_dsl {
    ($rust_crate:literal, $rust_module:literal) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, serde::Serialize, serde::Deserialize)]
        pub enum LiveProviderControlKind {
            #[default]
            Started,
            Diagnostic,
        }

        pub use meerkat_core::live_execution::observation::LiveProviderControlRefusal;

        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, serde::Serialize, serde::Deserialize)]
        pub enum LiveVoiceUsageKind {
            #[default]
            Periodic,
            Final,
            Invalid,
            ObservationClosed,
        }

        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, serde::Serialize, serde::Deserialize)]
        pub enum LiveVoiceUsageDispute {
            #[default]
            None,
            Regression,
            InvalidDuration,
            ConflictingFinal,
        }

        meerkat_machine_dsl::machine! {
            machine LiveTranscriptMachine {
                version: 7,
                rust: $rust_crate / $rust_module,

                state {
                    lifecycle_phase: LiveTranscriptOwnerPhase,
                    ingress_open: bool,
                    ingress_generation: u64,
                    channels: Set<String>,
                    accepting_channels: Set<String>,
                    source_accepting_channels: Set<String>,
                    activation_sequences: Map<String, u64>,
                    receive_ordinals: Map<String, u64>,
                    durable_watermarks: Map<String, u64>,
                    reservation_frontiers: Map<String, u64>,
                    gap_channels: Map<u64, String>,
                    control_credit_records: Map<String, u64>,
                    control_credit_bytes: Map<String, u64>,
                    control_maximum_record_charge: Map<String, u64>,
                    control_spent_records: Map<String, u64>,
                    control_spent_bytes: Map<String, u64>,
                    voice_channels: Set<String>,
                    voice_observation_open: Set<String>,
                    voice_usage_observed: Set<String>,
                    voice_final_observed: Set<String>,
                    voice_seconds_bits: Map<String, u64>,
                    voice_usage_disputes: Map<String, Enum<LiveVoiceUsageDispute>>,
                    voice_usage_digests: Map<String, String>,
                    voice_usage_sequences: Map<String, u64>,
                    provider_start_digests: Map<String, String>,
                    provider_control_sequences: Map<String, u64>,
                    provider_control_channels: Map<String, String>,
                }

                init(Ready) {
                    ingress_open = true,
                    ingress_generation = 1,
                    channels = EmptySet,
                    accepting_channels = EmptySet,
                    source_accepting_channels = EmptySet,
                    activation_sequences = EmptyMap,
                    receive_ordinals = EmptyMap,
                    durable_watermarks = EmptyMap,
                    reservation_frontiers = EmptyMap,
                    gap_channels = EmptyMap,
                    control_credit_records = EmptyMap,
                    control_credit_bytes = EmptyMap,
                    control_maximum_record_charge = EmptyMap,
                    control_spent_records = EmptyMap,
                    control_spent_bytes = EmptyMap,
                    voice_channels = EmptySet,
                    voice_observation_open = EmptySet,
                    voice_usage_observed = EmptySet,
                    voice_final_observed = EmptySet,
                    voice_seconds_bits = EmptyMap,
                    voice_usage_disputes = EmptyMap,
                    voice_usage_digests = EmptyMap,
                    voice_usage_sequences = EmptyMap,
                    provider_start_digests = EmptyMap,
                    provider_control_sequences = EmptyMap,
                    provider_control_channels = EmptyMap,
                }

                phase LiveTranscriptOwnerPhase { Ready }

                input LiveTranscriptInput {
                    ActivateChannel {
                        channel: String, sequence: u64, ingress_generation: u64,
                        credit_records: u64, credit_bytes: u64, maximum_record_charge: u64,
                        voice_accounting: bool,
                    },
                    AppendObservation {
                        channel: String, sequence: u64, receive_ordinal: u64,
                        ingress_generation: u64,
                    },
                    RecordKnownGap {
                        channel: String, sequence: u64,
                        after_received: u64, through_received: u64,
                        ingress_generation: u64, record_bytes: u64,
                    },
                    RecoverUnknownTail {
                        channel: String, sequence: u64, record_bytes: u64,
                    },
                    CloseChannel {
                        channel: String, sequence: u64, record_bytes: u64,
                    },
                    CloseIngress { ingress_generation: u64 },
                    CloseCurrentIngress {},
                    ObserveSourceIngress { channel: String },
                    FenceChannelSources { channel: String },
                    ReservePrefix {
                        channel: String, after: u64, through: u64,
                        received_through: u64, ingress_generation: u64,
                    },
                    SelectExplicitRange {
                        channel: String, after: u64, through: u64,
                        ingress_generation: u64,
                    },
                    FenceKnownReceiveTail {
                        channel: String, sequence: u64,
                        after_received: u64, through_received: u64, record_bytes: u64,
                    },
                    ObserveVoiceUsage {
                        channel: String, sequence: u64, kind: Enum<LiveVoiceUsageKind>,
                        seconds_bits: u64, digest: String, record_bytes: u64,
                    },
                    ObserveProviderControl {
                        channel: String, sequence: u64, kind: Enum<LiveProviderControlKind>,
                        digest: String, record_bytes: u64,
                    },
                }

                effect LiveTranscriptEffect {
                    ChannelActivated { channel: String, sequence: u64 },
                    ObservationAccepted {
                        channel: String, sequence: u64, receive_ordinal: u64,
                    },
                    KnownGapAccepted {
                        channel: String, sequence: u64,
                        after_received: u64, through_received: u64,
                    },
                    UnknownTailFenced { channel: String, sequence: u64 },
                    ChannelIngressClosed { channel: String, sequence: u64 },
                    IngressClosed { ingress_generation: u64 },
                    AwaitingObservationDurability { channel: String },
                    RangeSelected {
                        channel: String, after: u64, through: u64, discontinuous: bool,
                    },
                    KnownTailFenced {
                        channel: String, sequence: u64,
                        after_received: u64, through_received: u64,
                    },
                    VoiceUsageRecorded { channel: String, sequence: u64 },
                    VoiceUsageUnchanged { channel: String, sequence: u64 },
                    SourceIngressObserved { channel: String, ingress_open: bool },
                    ChannelSourcesFenced { channel: String },
                    ProviderControlRecorded { channel: String, digest: String, sequence: u64 },
                    ProviderControlUnchanged { channel: String, digest: String, sequence: u64 },
                    ProviderControlRefused { channel: String, reason: Enum<LiveProviderControlRefusal> },
                }

                disposition ChannelActivated => local seam SurfaceResultAlignment,
                disposition ObservationAccepted => local seam SurfaceResultAlignment,
                disposition KnownGapAccepted => local seam SurfaceResultAlignment,
                disposition UnknownTailFenced => local seam SurfaceResultAlignment,
                disposition ChannelIngressClosed => local seam SurfaceResultAlignment,
                disposition IngressClosed => local seam SurfaceResultAlignment,
                disposition AwaitingObservationDurability => local seam SurfaceResultAlignment,
                disposition RangeSelected => local seam SurfaceResultAlignment,
                disposition KnownTailFenced => local seam SurfaceResultAlignment,
                disposition VoiceUsageRecorded => local seam SurfaceResultAlignment,
                disposition VoiceUsageUnchanged => local seam SurfaceResultAlignment,
                disposition SourceIngressObserved => local seam SurfaceResultAlignment,
                disposition ChannelSourcesFenced => local seam SurfaceResultAlignment,
                disposition ProviderControlRecorded => local seam SurfaceResultAlignment,
                disposition ProviderControlUnchanged => local seam SurfaceResultAlignment,
                disposition ProviderControlRefused => local seam SurfaceResultAlignment,

                transition ObserveRepeatedProviderControl {
                    on input ObserveProviderControl { channel, sequence, kind, digest, record_bytes }
                    guard {
                        self.voice_channels.contains(channel)
                        && self.provider_control_channels.get_cloned(digest) == Some(channel)
                    }
                    update {}
                    to Ready
                    emit ProviderControlUnchanged {
                        channel: channel, digest: digest,
                        sequence: self.provider_control_sequences.get_cloned(digest).get("value")
                    }
                }

                transition RefuseConflictingProviderStart {
                    on input ObserveProviderControl { channel, sequence, kind, digest, record_bytes }
                    guard {
                        self.voice_channels.contains(channel)
                        && !self.provider_control_channels.contains_key(digest)
                        && kind == LiveProviderControlKind::Started
                        && self.provider_start_digests.contains_key(channel)
                    }
                    update {}
                    to Ready
                    emit ProviderControlRefused { channel: channel, reason: LiveProviderControlRefusal::IdentityConflict }
                }

                transition RefuseLateProviderControl {
                    on input ObserveProviderControl { channel, sequence, kind, digest, record_bytes }
                    guard {
                        self.voice_channels.contains(channel)
                        && !self.provider_control_channels.contains_key(digest)
                        && (kind != LiveProviderControlKind::Started || !self.provider_start_digests.contains_key(channel))
                        && !self.voice_observation_open.contains(channel)
                    }
                    update {}
                    to Ready
                    emit ProviderControlRefused { channel: channel, reason: LiveProviderControlRefusal::ObservationClosed }
                }

                transition RecordProviderControl {
                    on input ObserveProviderControl { channel, sequence, kind, digest, record_bytes }
                    guard {
                        self.voice_observation_open.contains(channel)
                        && digest != "" && !self.provider_control_channels.contains_key(digest)
                        && (kind != LiveProviderControlKind::Started || !self.provider_start_digests.contains_key(channel))
                        && sequence > self.activation_sequences.get_cloned(channel).get("value")
                        && record_bytes > 0
                        && record_bytes <= self.control_maximum_record_charge.get_cloned(channel).get("value")
                        && self.control_credit_records.get_cloned(channel).get("value")
                            - self.control_spent_records.get_cloned(channel).get("value") > 3
                        && self.control_credit_bytes.get_cloned(channel).get("value")
                            - self.control_spent_bytes.get_cloned(channel).get("value")
                            >= 3 * self.control_maximum_record_charge.get_cloned(channel).get("value")
                        && record_bytes <= self.control_credit_bytes.get_cloned(channel).get("value")
                            - self.control_spent_bytes.get_cloned(channel).get("value")
                            - 3 * self.control_maximum_record_charge.get_cloned(channel).get("value")
                    }
                    update {
                        self.provider_control_channels.insert(digest, channel);
                        self.provider_control_sequences.insert(digest, sequence);
                        if kind == LiveProviderControlKind::Started {
                            self.provider_start_digests.insert(channel, digest);
                        }
                        self.control_spent_records.insert(channel, self.control_spent_records.get_cloned(channel).get("value") + 1);
                        self.control_spent_bytes.insert(channel, self.control_spent_bytes.get_cloned(channel).get("value") + record_bytes);
                    }
                    to Ready
                    emit ProviderControlRecorded { channel: channel, digest: digest, sequence: sequence }
                }

                transition RefuseProviderControlCapacity {
                    on input ObserveProviderControl { channel, sequence, kind, digest, record_bytes }
                    guard {
                        self.voice_observation_open.contains(channel)
                        && digest != "" && !self.provider_control_channels.contains_key(digest)
                        && (kind != LiveProviderControlKind::Started || !self.provider_start_digests.contains_key(channel))
                        && sequence > self.activation_sequences.get_cloned(channel).get("value")
                        && record_bytes > 0
                        && (record_bytes > self.control_maximum_record_charge.get_cloned(channel).get("value")
                            || self.control_credit_records.get_cloned(channel).get("value")
                                - self.control_spent_records.get_cloned(channel).get("value") <= 3
                            || self.control_credit_bytes.get_cloned(channel).get("value")
                                - self.control_spent_bytes.get_cloned(channel).get("value")
                                < 3 * self.control_maximum_record_charge.get_cloned(channel).get("value")
                            || record_bytes > self.control_credit_bytes.get_cloned(channel).get("value")
                                - self.control_spent_bytes.get_cloned(channel).get("value")
                                - 3 * self.control_maximum_record_charge.get_cloned(channel).get("value"))
                    }
                    update {}
                    to Ready
                    emit ProviderControlRefused { channel: channel, reason: LiveProviderControlRefusal::Capacity }
                }

                transition ObserveExactSourceIngress {
                    on input ObserveSourceIngress { channel }
                    guard { self.channels.contains(channel) }
                    update {}
                    to Ready
                    emit SourceIngressObserved {
                        channel: channel,
                        ingress_open: self.ingress_open && self.source_accepting_channels.contains(channel)
                    }
                }

                transition FenceExactChannelSources {
                    on input FenceChannelSources { channel }
                    guard { self.channels.contains(channel) }
                    update { self.source_accepting_channels.remove(channel); }
                    to Ready
                    emit ChannelSourcesFenced { channel: channel }
                }

                transition ActivateFreshChannel {
                    on input ActivateChannel {
                        channel, sequence, ingress_generation,
                        credit_records, credit_bytes, maximum_record_charge, voice_accounting
                    }
                    guard {
                        self.ingress_open && ingress_generation == self.ingress_generation
                        && channel != "" && !self.channels.contains(channel)
                        && sequence > 0 && credit_records > 1 && maximum_record_charge > 0
                        && (!voice_accounting || credit_records > 2)
                        && maximum_record_charge <= credit_bytes / credit_records
                    }
                    update {
                        self.channels.insert(channel);
                        self.accepting_channels.insert(channel);
                        self.source_accepting_channels.insert(channel);
                        self.activation_sequences.insert(channel, sequence);
                        self.receive_ordinals.insert(channel, 0);
                        self.durable_watermarks.insert(channel, sequence);
                        self.reservation_frontiers.insert(channel, sequence);
                        self.control_credit_records.insert(channel, credit_records);
                        self.control_credit_bytes.insert(channel, credit_bytes);
                        self.control_maximum_record_charge.insert(channel, maximum_record_charge);
                        self.control_spent_records.insert(channel, 0);
                        self.control_spent_bytes.insert(channel, 0);
                        if voice_accounting {
                            self.voice_channels.insert(channel);
                            self.voice_observation_open.insert(channel);
                            self.voice_seconds_bits.insert(channel, 0);
                            self.voice_usage_disputes.insert(channel, LiveVoiceUsageDispute::None);
                            self.voice_usage_digests.insert(channel, "");
                            self.voice_usage_sequences.insert(channel, 0);
                        }
                    }
                    to Ready
                    emit ChannelActivated { channel: channel, sequence: sequence }
                }

                transition ObserveRepeatedVoiceUsage {
                    on input ObserveVoiceUsage { channel, sequence, kind, seconds_bits, digest, record_bytes }
                    guard {
                        self.voice_channels.contains(channel)
                        && digest != ""
                        && self.voice_usage_digests.get_cloned(channel) == Some(digest)
                    }
                    update {}
                    to Ready
                    emit VoiceUsageUnchanged {
                        channel: channel,
                        sequence: self.voice_usage_sequences.get_cloned(channel).get("value")
                    }
                }

                transition ReconcileVoiceUsage {
                    on input ObserveVoiceUsage { channel, sequence, kind, seconds_bits, digest, record_bytes }
                    guard {
                        self.voice_observation_open.contains(channel)
                        && digest != "" && self.voice_usage_digests.get_cloned(channel) != Some(digest)
                        && sequence > self.voice_usage_sequences.get_cloned(channel).get("value")
                        && seconds_bits <= 9218868437227405311
                        && (kind != LiveVoiceUsageKind::ObservationClosed || !self.accepting_channels.contains(channel))
                        && self.control_spent_records.get_cloned(channel).get("value")
                            < self.control_credit_records.get_cloned(channel).get("value")
                        && record_bytes > 0
                        && record_bytes <= self.control_maximum_record_charge.get_cloned(channel).get("value")
                        && record_bytes <= self.control_credit_bytes.get_cloned(channel).get("value")
                            - self.control_spent_bytes.get_cloned(channel).get("value")
                        && (kind == LiveVoiceUsageKind::Periodic
                            || kind == LiveVoiceUsageKind::ObservationClosed
                            || (!self.accepting_channels.contains(channel)
                                && self.control_credit_records.get_cloned(channel).get("value")
                                    - self.control_spent_records.get_cloned(channel).get("value") > 1
                                && self.control_credit_bytes.get_cloned(channel).get("value")
                                    - self.control_spent_bytes.get_cloned(channel).get("value")
                                    - record_bytes >= self.control_maximum_record_charge.get_cloned(channel).get("value"))
                            || (self.accepting_channels.contains(channel)
                                && self.control_credit_records.get_cloned(channel).get("value")
                                    - self.control_spent_records.get_cloned(channel).get("value") > 2
                                && self.control_credit_bytes.get_cloned(channel).get("value")
                                    - self.control_spent_bytes.get_cloned(channel).get("value")
                                    - record_bytes >= 2 * self.control_maximum_record_charge.get_cloned(channel).get("value")))
                        && (kind != LiveVoiceUsageKind::Invalid || !self.accepting_channels.contains(channel)
                            || (self.control_credit_records.get_cloned(channel).get("value")
                                    - self.control_spent_records.get_cloned(channel).get("value") > 3
                                && self.control_credit_bytes.get_cloned(channel).get("value")
                                    - self.control_spent_bytes.get_cloned(channel).get("value")
                                    - record_bytes >= 3 * self.control_maximum_record_charge.get_cloned(channel).get("value")))
                    }
                    update {
                        if kind == LiveVoiceUsageKind::ObservationClosed {
                            self.voice_observation_open.remove(channel);
                        } else {
                            if kind == LiveVoiceUsageKind::Invalid {
                                self.voice_usage_disputes.insert(channel, LiveVoiceUsageDispute::InvalidDuration);
                            } else {
                                if self.voice_final_observed.contains(channel) {
                                    if seconds_bits != self.voice_seconds_bits.get_cloned(channel).get("value") {
                                        self.voice_usage_disputes.insert(channel, LiveVoiceUsageDispute::ConflictingFinal);
                                    }
                                } else {
                                    if self.voice_usage_observed.contains(channel)
                                        && seconds_bits < self.voice_seconds_bits.get_cloned(channel).get("value") {
                                        self.voice_usage_disputes.insert(channel, LiveVoiceUsageDispute::Regression);
                                    } else {
                                        self.voice_seconds_bits.insert(channel, seconds_bits);
                                        self.voice_usage_observed.insert(channel);
                                        if kind == LiveVoiceUsageKind::Final {
                                            self.voice_final_observed.insert(channel);
                                        }
                                    }
                                }
                            }
                        }
                        self.voice_usage_digests.insert(channel, digest);
                        self.voice_usage_sequences.insert(channel, sequence);
                        if kind != LiveVoiceUsageKind::Periodic {
                            self.control_spent_records.insert(channel, self.control_spent_records.get_cloned(channel).get("value") + 1);
                            self.control_spent_bytes.insert(channel, self.control_spent_bytes.get_cloned(channel).get("value") + record_bytes);
                        }
                    }
                    to Ready
                    emit VoiceUsageRecorded { channel: channel, sequence: sequence }
                }

                transition AcceptNextObservation {
                    on input AppendObservation { channel, sequence, receive_ordinal, ingress_generation }
                    guard {
                        self.ingress_open && ingress_generation == self.ingress_generation
                        && self.accepting_channels.contains(channel)
                        && sequence > self.durable_watermarks.get_cloned(channel).get("value")
                        && receive_ordinal > self.receive_ordinals.get_cloned(channel).get("value")
                        && receive_ordinal - self.receive_ordinals.get_cloned(channel).get("value") == 1
                    }
                    update {
                        self.receive_ordinals.insert(channel, receive_ordinal);
                        self.durable_watermarks.insert(channel, sequence);
                    }
                    to Ready
                    emit ObservationAccepted {
                        channel: channel, sequence: sequence, receive_ordinal: receive_ordinal
                    }
                }

                transition AcceptKnownReceiveGap {
                    on input RecordKnownGap {
                        channel, sequence, after_received, through_received,
                        ingress_generation, record_bytes
                    }
                    guard {
                        self.ingress_open && ingress_generation == self.ingress_generation
                        && self.accepting_channels.contains(channel)
                        && (!self.voice_observation_open.contains(channel)
                            || (self.control_credit_records.get_cloned(channel).get("value")
                                    - self.control_spent_records.get_cloned(channel).get("value") > 3
                                && self.control_credit_bytes.get_cloned(channel).get("value")
                                    - self.control_spent_bytes.get_cloned(channel).get("value")
                                    >= 4 * self.control_maximum_record_charge.get_cloned(channel).get("value")))
                        && sequence > self.durable_watermarks.get_cloned(channel).get("value")
                        && !self.gap_channels.contains_key(sequence)
                        && after_received == self.receive_ordinals.get_cloned(channel).get("value")
                        && through_received > after_received
                        && self.control_credit_records.get_cloned(channel).get("value")
                            - self.control_spent_records.get_cloned(channel).get("value") > 1
                        && record_bytes > 0
                        && record_bytes <= self.control_maximum_record_charge.get_cloned(channel).get("value")
                        && self.control_credit_bytes.get_cloned(channel).get("value")
                            - self.control_spent_bytes.get_cloned(channel).get("value")
                            >= self.control_maximum_record_charge.get_cloned(channel).get("value")
                        && record_bytes <= self.control_credit_bytes.get_cloned(channel).get("value")
                            - self.control_spent_bytes.get_cloned(channel).get("value")
                            - self.control_maximum_record_charge.get_cloned(channel).get("value")
                    }
                    update {
                        self.receive_ordinals.insert(channel, through_received);
                        self.durable_watermarks.insert(channel, sequence);
                        self.gap_channels.insert(sequence, channel);
                        self.control_spent_records.insert(channel, self.control_spent_records.get_cloned(channel).get("value") + 1);
                        self.control_spent_bytes.insert(channel, self.control_spent_bytes.get_cloned(channel).get("value") + record_bytes);
                    }
                    to Ready
                    emit KnownGapAccepted {
                        channel: channel, sequence: sequence,
                        after_received: after_received, through_received: through_received
                    }
                }

                transition FenceUnknownCrashTail {
                    on input RecoverUnknownTail { channel, sequence, record_bytes }
                    guard {
                        self.accepting_channels.contains(channel)
                        && (!self.voice_observation_open.contains(channel)
                            || (self.control_credit_records.get_cloned(channel).get("value")
                                    - self.control_spent_records.get_cloned(channel).get("value") > 1
                                && self.control_credit_bytes.get_cloned(channel).get("value")
                                    - self.control_spent_bytes.get_cloned(channel).get("value")
                                    >= 2 * self.control_maximum_record_charge.get_cloned(channel).get("value")))
                        && sequence > self.durable_watermarks.get_cloned(channel).get("value")
                        && !self.gap_channels.contains_key(sequence)
                        && self.control_spent_records.get_cloned(channel).get("value")
                            < self.control_credit_records.get_cloned(channel).get("value")
                        && record_bytes > 0
                        && record_bytes <= self.control_maximum_record_charge.get_cloned(channel).get("value")
                        && record_bytes <= self.control_credit_bytes.get_cloned(channel).get("value")
                            - self.control_spent_bytes.get_cloned(channel).get("value")
                    }

                    update {
                        self.accepting_channels.remove(channel);
                        self.source_accepting_channels.remove(channel);
                        self.durable_watermarks.insert(channel, sequence);
                        self.gap_channels.insert(sequence, channel);
                        self.control_spent_records.insert(channel, self.control_spent_records.get_cloned(channel).get("value") + 1);
                        self.control_spent_bytes.insert(channel, self.control_spent_bytes.get_cloned(channel).get("value") + record_bytes);
                    }
                    to Ready
                    emit UnknownTailFenced { channel: channel, sequence: sequence }
                }

                transition FenceKnownUncommittedTail {
                    on input FenceKnownReceiveTail {
                        channel, sequence, after_received, through_received, record_bytes
                    }
                    guard {
                        self.accepting_channels.contains(channel)
                        && (!self.voice_observation_open.contains(channel)
                            || (self.control_credit_records.get_cloned(channel).get("value")
                                    - self.control_spent_records.get_cloned(channel).get("value") > 1
                                && self.control_credit_bytes.get_cloned(channel).get("value")
                                    - self.control_spent_bytes.get_cloned(channel).get("value")
                                    >= 2 * self.control_maximum_record_charge.get_cloned(channel).get("value")))
                        && sequence > self.durable_watermarks.get_cloned(channel).get("value")
                        && !self.gap_channels.contains_key(sequence)
                        && after_received == self.receive_ordinals.get_cloned(channel).get("value")
                        && through_received > after_received
                        && self.control_spent_records.get_cloned(channel).get("value")
                            < self.control_credit_records.get_cloned(channel).get("value")
                        && record_bytes > 0
                        && record_bytes <= self.control_maximum_record_charge.get_cloned(channel).get("value")
                        && record_bytes <= self.control_credit_bytes.get_cloned(channel).get("value")
                            - self.control_spent_bytes.get_cloned(channel).get("value")
                    }
                    update {
                        self.accepting_channels.remove(channel);
                        self.source_accepting_channels.remove(channel);
                        self.receive_ordinals.insert(channel, through_received);
                        self.durable_watermarks.insert(channel, sequence);
                        self.gap_channels.insert(sequence, channel);
                        self.control_spent_records.insert(channel, self.control_spent_records.get_cloned(channel).get("value") + 1);
                        self.control_spent_bytes.insert(channel, self.control_spent_bytes.get_cloned(channel).get("value") + record_bytes);
                    }
                    to Ready
                    emit KnownTailFenced {
                        channel: channel, sequence: sequence,
                        after_received: after_received, through_received: through_received
                    }
                }

                transition CloseExactChannelIngress {
                    on input CloseChannel { channel, sequence, record_bytes }
                    guard {
                        self.accepting_channels.contains(channel)
                        && (!self.voice_observation_open.contains(channel)
                            || (self.control_credit_records.get_cloned(channel).get("value")
                                    - self.control_spent_records.get_cloned(channel).get("value") > 1
                                && self.control_credit_bytes.get_cloned(channel).get("value")
                                    - self.control_spent_bytes.get_cloned(channel).get("value")
                                    >= 2 * self.control_maximum_record_charge.get_cloned(channel).get("value")))
                        && sequence > self.durable_watermarks.get_cloned(channel).get("value")
                        && self.control_spent_records.get_cloned(channel).get("value")
                            < self.control_credit_records.get_cloned(channel).get("value")
                        && record_bytes > 0
                        && record_bytes <= self.control_maximum_record_charge.get_cloned(channel).get("value")
                        && record_bytes <= self.control_credit_bytes.get_cloned(channel).get("value")
                            - self.control_spent_bytes.get_cloned(channel).get("value")
                    }
                    update {
                        self.accepting_channels.remove(channel);
                        self.source_accepting_channels.remove(channel);
                        self.durable_watermarks.insert(channel, sequence);
                        self.control_spent_records.insert(channel, self.control_spent_records.get_cloned(channel).get("value") + 1);
                        self.control_spent_bytes.insert(channel, self.control_spent_bytes.get_cloned(channel).get("value") + record_bytes);
                    }
                    to Ready
                    emit ChannelIngressClosed { channel: channel, sequence: sequence }
                }

                transition FenceSessionIngress {
                    on input CloseIngress { ingress_generation }
                    guard { self.ingress_open && ingress_generation > self.ingress_generation }
                    update {
                        self.ingress_open = false;
                        self.ingress_generation = ingress_generation;
                    }
                    to Ready
                    emit IngressClosed { ingress_generation: ingress_generation }
                }

                transition FenceCurrentSessionIngress {
                    on input CloseCurrentIngress {}
                    guard { self.ingress_open && self.ingress_generation < 18446744073709551615 }
                    update {
                        self.ingress_open = false;
                        self.ingress_generation += 1;
                    }
                    to Ready
                    emit IngressClosed { ingress_generation: self.ingress_generation }
                }

                transition ObserveClosedSessionIngress {
                    on input CloseCurrentIngress {}
                    guard { !self.ingress_open }
                    update {}
                    to Ready
                    emit IngressClosed { ingress_generation: self.ingress_generation }
                }

                transition AwaitUncommittedObservations {
                    on input ReservePrefix { channel, after, through, received_through, ingress_generation }
                    guard {
                        self.ingress_open && ingress_generation == self.ingress_generation
                        && self.source_accepting_channels.contains(channel)
                        && after == self.reservation_frontiers.get_cloned(channel).get("value")
                        && through == self.durable_watermarks.get_cloned(channel).get("value")
                        && received_through > self.receive_ordinals.get_cloned(channel).get("value")
                    }
                    update {}
                    to Ready
                    emit AwaitingObservationDurability { channel: channel }
                }

                transition ReserveDurablePrefix {
                    on input ReservePrefix { channel, after, through, received_through, ingress_generation }
                    guard {
                        self.ingress_open && ingress_generation == self.ingress_generation
                        && self.source_accepting_channels.contains(channel)
                        && after == self.reservation_frontiers.get_cloned(channel).get("value")
                        && through == self.durable_watermarks.get_cloned(channel).get("value")
                        && received_through == self.receive_ordinals.get_cloned(channel).get("value")
                    }
                    update { self.reservation_frontiers.insert(channel, through); }
                    to Ready
                    emit RangeSelected {
                        channel: channel, after: after, through: through,
                        discontinuous: !for_all(gap in self.gap_channels.keys(),
                            self.gap_channels.get_cloned(gap) != Some(channel)
                            || gap <= after || gap > through)
                    }
                }

                transition SelectDurableExplicitRange {
                    on input SelectExplicitRange { channel, after, through, ingress_generation }
                    guard {
                        self.ingress_open && ingress_generation == self.ingress_generation
                        && self.source_accepting_channels.contains(channel)
                        && after >= self.activation_sequences.get_cloned(channel).get("value")
                        && through >= after
                        && through <= self.durable_watermarks.get_cloned(channel).get("value")
                    }
                    update {}
                    to Ready
                    emit RangeSelected {
                        channel: channel, after: after, through: through,
                        discontinuous: !for_all(gap in self.gap_channels.keys(),
                            self.gap_channels.get_cloned(gap) != Some(channel)
                            || gap <= after || gap > through)
                    }
                }

                invariant channel_coverage_is_complete {
                    self.activation_sequences.keys() == self.channels
                    && self.receive_ordinals.keys() == self.channels
                    && self.durable_watermarks.keys() == self.channels
                    && self.reservation_frontiers.keys() == self.channels
                    && for_all(channel in self.accepting_channels, self.channels.contains(channel))
                    && for_all(channel in self.source_accepting_channels, self.accepting_channels.contains(channel))
                    && for_all(channel in self.channels,
                        self.activation_sequences.get_cloned(channel).get("value") > 0
                        && self.reservation_frontiers.get_cloned(channel).get("value")
                            >= self.activation_sequences.get_cloned(channel).get("value")
                        && self.reservation_frontiers.get_cloned(channel).get("value")
                            <= self.durable_watermarks.get_cloned(channel).get("value"))
                    && for_all(gap in self.gap_channels.keys(),
                        self.channels.contains(self.gap_channels.get_cloned(gap).get("value"))
                        && gap > self.activation_sequences.get_cloned(self.gap_channels.get_cloned(gap).get("value")).get("value")
                        && gap <= self.durable_watermarks.get_cloned(self.gap_channels.get_cloned(gap).get("value")).get("value"))
                }

                invariant control_capacity_is_retained {
                    self.ingress_generation > 0
                    && self.control_credit_records.keys() == self.channels
                    && self.control_credit_bytes.keys() == self.channels
                    && self.control_maximum_record_charge.keys() == self.channels
                    && self.control_spent_records.keys() == self.channels
                    && self.control_spent_bytes.keys() == self.channels
                    && for_all(channel in self.channels,
                        self.control_credit_records.get_cloned(channel).get("value") > 1
                        && self.control_maximum_record_charge.get_cloned(channel).get("value") > 0
                        && self.control_spent_records.get_cloned(channel).get("value")
                            <= self.control_credit_records.get_cloned(channel).get("value")
                        && self.control_spent_bytes.get_cloned(channel).get("value")
                            <= self.control_credit_bytes.get_cloned(channel).get("value")
                        && (!self.accepting_channels.contains(channel)
                            || (self.control_spent_records.get_cloned(channel).get("value")
                                    < self.control_credit_records.get_cloned(channel).get("value")
                                && self.control_credit_bytes.get_cloned(channel).get("value")
                                    - self.control_spent_bytes.get_cloned(channel).get("value")
                                    >= self.control_maximum_record_charge.get_cloned(channel).get("value"))))
                }

                invariant voice_accounting_remains_channel_scoped {
                    self.voice_seconds_bits.keys() == self.voice_channels
                    && self.voice_usage_disputes.keys() == self.voice_channels
                    && self.voice_usage_digests.keys() == self.voice_channels
                    && self.voice_usage_sequences.keys() == self.voice_channels
                    && for_all(channel in self.voice_channels, self.channels.contains(channel)
                        && self.voice_seconds_bits.get_cloned(channel).get("value") <= 9218868437227405311)
                    && for_all(channel in self.voice_observation_open, self.voice_channels.contains(channel))
                    && for_all(channel in self.voice_observation_open,
                        self.control_spent_records.get_cloned(channel).get("value")
                            < self.control_credit_records.get_cloned(channel).get("value")
                        && self.control_credit_bytes.get_cloned(channel).get("value")
                            - self.control_spent_bytes.get_cloned(channel).get("value")
                            >= self.control_maximum_record_charge.get_cloned(channel).get("value"))
                    && for_all(channel in self.voice_usage_observed, self.voice_channels.contains(channel))
                    && for_all(channel in self.voice_final_observed, self.voice_usage_observed.contains(channel))
                }

                invariant provider_controls_retain_exact_receipts {
                    self.provider_control_sequences.keys() == self.provider_control_channels.keys()
                    && for_all(digest in self.provider_control_channels.keys(),
                        digest != ""
                        && self.voice_channels.contains(self.provider_control_channels.get_cloned(digest).get("value"))
                        && self.provider_control_sequences.get_cloned(digest).get("value")
                            > self.activation_sequences.get_cloned(self.provider_control_channels.get_cloned(digest).get("value")).get("value"))
                    && for_all(channel in self.provider_start_digests.keys(),
                        self.provider_control_channels.get_cloned(self.provider_start_digests.get_cloned(channel).get("value")) == Some(channel))
                }
            }
        }
    };
}

live_transcript_catalog_machine_dsl!("self", "catalog::dsl::live_transcript");
