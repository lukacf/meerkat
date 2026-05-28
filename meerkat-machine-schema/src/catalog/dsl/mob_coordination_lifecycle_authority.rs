use meerkat_machine_dsl::machine;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct WorkIntentId(pub String);

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct ResourceClaimId(pub String);

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct CoordinationResourceRef(pub String);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum MobCoordinationWorkIntentStatus {
    #[default]
    Planned,
    Active,
    Blocked,
    Completed,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum MobCoordinationResourceClaimStatus {
    #[default]
    Active,
    Released,
    Expired,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum MobCoordinationResourceClaimKind {
    #[default]
    Advisory,
    SoftReservation,
    Exclusive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum MobCoordinationEventKind {
    #[default]
    WorkIntentRecorded,
    WorkIntentStatusChanged,
    ResourceClaimRecorded,
    ResourceClaimStatusChanged,
    ResourceClaimOverlapObserved,
}

machine! {
    machine MobCoordinationLifecycleAuthorityMachine {
        version: 1,
        rust: "self" / "catalog::dsl::mob_coordination_lifecycle_authority",

        state {
            lifecycle_phase: MobCoordinationLifecycleAuthorityPhase,
            initial_record_revision: u64,
            initial_event_sequence: u64,
        }

        init(Ready) {
            initial_record_revision = 1,
            initial_event_sequence = 1,
        }

        terminal []

        phase MobCoordinationLifecycleAuthorityPhase {
            Ready,
        }

        input MobCoordinationLifecycleAuthorityInput {
            AuthorizeInitialRecordRevision,
            AdvanceRecordRevision {
                current_revision: u64,
                expected_revision: u64,
            },
            AuthorizeInitialEventSequence,
            RecordWorkIntent {
                intent_id: WorkIntentId,
                requested_status: Enum<MobCoordinationWorkIntentStatus>,
                owner_principal_key: Option<String>,
                owner_agent_identity: Option<String>,
                owning_mob_ref_matches: bool,
                resource_tokens: Seq<CoordinationResourceRef>,
                summary_present: bool,
                metadata_public: bool,
                expires_at_utc_ms: Option<u64>,
                already_exists: bool,
            },
            RecordResourceClaim {
                claim_id: ResourceClaimId,
                requested_kind: Enum<MobCoordinationResourceClaimKind>,
                requested_status: Enum<MobCoordinationResourceClaimStatus>,
                owner_principal_key: Option<String>,
                owner_agent_identity: Option<String>,
                owning_mob_ref_matches: bool,
                resource_tokens: Seq<CoordinationResourceRef>,
                metadata_public: bool,
                expires_at_utc_ms: Option<u64>,
                already_exists: bool,
            },
            PublishWorkIntentRecordedEvent { current_next_sequence: u64, intent_id: WorkIntentId },
            PublishWorkIntentStatusChangedEvent {
                current_next_sequence: u64,
                intent_id: WorkIntentId,
                status: Enum<MobCoordinationWorkIntentStatus>,
            },
            PublishResourceClaimRecordedEvent {
                current_next_sequence: u64,
                claim_id: ResourceClaimId,
                kind: Enum<MobCoordinationResourceClaimKind>,
            },
            PublishResourceClaimStatusChangedEvent {
                current_next_sequence: u64,
                claim_id: ResourceClaimId,
                status: Enum<MobCoordinationResourceClaimStatus>,
            },
            PublishResourceClaimOverlapObservedEvent {
                current_next_sequence: u64,
                claim_id: ResourceClaimId,
                overlap_ids: Seq<ResourceClaimId>,
            },
            RecordWorkIntentStatus { requested_status: Enum<MobCoordinationWorkIntentStatus> },
            UpdateWorkIntentStatus {
                requested_status: Enum<MobCoordinationWorkIntentStatus>,
                is_expired: bool,
            },
            RecordResourceClaimStatus { requested_status: Enum<MobCoordinationResourceClaimStatus> },
            UpdateResourceClaimStatus {
                requested_status: Enum<MobCoordinationResourceClaimStatus>,
                is_expired: bool,
            },
        }

        effect MobCoordinationLifecycleAuthorityEffect {
            RecordRevisionAuthorized { revision: u64 },
            EventCursorAuthorized { next_sequence: u64 },
            WorkIntentRecordAuthorized {
                intent_id: WorkIntentId,
                status: Enum<MobCoordinationWorkIntentStatus>,
                revision: u64,
                owner_principal_key: Option<String>,
                owner_agent_identity: Option<String>,
                owning_mob_ref_matches: bool,
                resource_tokens: Seq<CoordinationResourceRef>,
                summary_present: bool,
                metadata_public: bool,
                expires_at_utc_ms: Option<u64>,
            },
            ResourceClaimRecordAuthorized {
                claim_id: ResourceClaimId,
                kind: Enum<MobCoordinationResourceClaimKind>,
                status: Enum<MobCoordinationResourceClaimStatus>,
                revision: u64,
                owner_principal_key: Option<String>,
                owner_agent_identity: Option<String>,
                owning_mob_ref_matches: bool,
                resource_tokens: Seq<CoordinationResourceRef>,
                metadata_public: bool,
                expires_at_utc_ms: Option<u64>,
            },
            WorkIntentRecordedEventAuthorized {
                event_kind: Enum<MobCoordinationEventKind>,
                intent_id: WorkIntentId,
                sequence: u64,
                next_sequence: u64,
            },
            WorkIntentStatusChangedEventAuthorized {
                event_kind: Enum<MobCoordinationEventKind>,
                intent_id: WorkIntentId,
                status: Enum<MobCoordinationWorkIntentStatus>,
                sequence: u64,
                next_sequence: u64,
            },
            ResourceClaimRecordedEventAuthorized {
                event_kind: Enum<MobCoordinationEventKind>,
                claim_id: ResourceClaimId,
                kind: Enum<MobCoordinationResourceClaimKind>,
                sequence: u64,
                next_sequence: u64,
            },
            ResourceClaimStatusChangedEventAuthorized {
                event_kind: Enum<MobCoordinationEventKind>,
                claim_id: ResourceClaimId,
                status: Enum<MobCoordinationResourceClaimStatus>,
                sequence: u64,
                next_sequence: u64,
            },
            ResourceClaimOverlapObservedEventAuthorized {
                event_kind: Enum<MobCoordinationEventKind>,
                claim_id: ResourceClaimId,
                overlap_ids: Seq<ResourceClaimId>,
                sequence: u64,
                next_sequence: u64,
            },
            WorkIntentStatusAuthorized { status: Enum<MobCoordinationWorkIntentStatus> },
            ResourceClaimStatusAuthorized { status: Enum<MobCoordinationResourceClaimStatus> },
        }

        disposition RecordRevisionAuthorized => local,
        disposition EventCursorAuthorized => local,
        disposition WorkIntentRecordAuthorized => local,
        disposition ResourceClaimRecordAuthorized => local,
        disposition WorkIntentRecordedEventAuthorized => local,
        disposition WorkIntentStatusChangedEventAuthorized => local,
        disposition ResourceClaimRecordedEventAuthorized => local,
        disposition ResourceClaimStatusChangedEventAuthorized => local,
        disposition ResourceClaimOverlapObservedEventAuthorized => local,
        disposition WorkIntentStatusAuthorized => local,
        disposition ResourceClaimStatusAuthorized => local,

        transition AuthorizeInitialRecordRevision {
            on input AuthorizeInitialRecordRevision
            guard { self.lifecycle_phase == Phase::Ready }
            update {}
            to Ready
            emit RecordRevisionAuthorized { revision: self.initial_record_revision }
        }

        transition AdvanceRecordRevision {
            on input AdvanceRecordRevision { current_revision, expected_revision }
            guard { self.lifecycle_phase == Phase::Ready && current_revision == expected_revision && current_revision > 0 }
            update {}
            to Ready
            emit RecordRevisionAuthorized { revision: current_revision + 1 }
        }

        transition AuthorizeInitialEventSequence {
            on input AuthorizeInitialEventSequence
            guard { self.lifecycle_phase == Phase::Ready }
            update {}
            to Ready
            emit EventCursorAuthorized { next_sequence: self.initial_event_sequence }
        }

        transition RecordWorkIntent {
            on input RecordWorkIntent {
                intent_id,
                requested_status,
                owner_principal_key,
                owner_agent_identity,
                owning_mob_ref_matches,
                resource_tokens,
                summary_present,
                metadata_public,
                expires_at_utc_ms,
                already_exists,
            }
            guard {
                self.lifecycle_phase == Phase::Ready
                && already_exists == false
                && summary_present == true
                && metadata_public == true
                && owning_mob_ref_matches == true
                && resource_tokens.len() > 0
                && (owner_principal_key != None || owner_agent_identity != None)
            }
            update {}
            to Ready
            emit WorkIntentRecordAuthorized {
                intent_id: intent_id,
                status: requested_status,
                revision: self.initial_record_revision,
                owner_principal_key: owner_principal_key,
                owner_agent_identity: owner_agent_identity,
                owning_mob_ref_matches: owning_mob_ref_matches,
                resource_tokens: resource_tokens,
                summary_present: summary_present,
                metadata_public: metadata_public,
                expires_at_utc_ms: expires_at_utc_ms
            }
        }

        transition RecordResourceClaim {
            on input RecordResourceClaim {
                claim_id,
                requested_kind,
                requested_status,
                owner_principal_key,
                owner_agent_identity,
                owning_mob_ref_matches,
                resource_tokens,
                metadata_public,
                expires_at_utc_ms,
                already_exists,
            }
            guard {
                self.lifecycle_phase == Phase::Ready
                && already_exists == false
                && metadata_public == true
                && owning_mob_ref_matches == true
                && resource_tokens.len() > 0
                && (owner_principal_key != None || owner_agent_identity != None)
            }
            update {}
            to Ready
            emit ResourceClaimRecordAuthorized {
                claim_id: claim_id,
                kind: requested_kind,
                status: requested_status,
                revision: self.initial_record_revision,
                owner_principal_key: owner_principal_key,
                owner_agent_identity: owner_agent_identity,
                owning_mob_ref_matches: owning_mob_ref_matches,
                resource_tokens: resource_tokens,
                metadata_public: metadata_public,
                expires_at_utc_ms: expires_at_utc_ms
            }
        }

        transition PublishWorkIntentRecordedEvent {
            on input PublishWorkIntentRecordedEvent { current_next_sequence, intent_id }
            guard { self.lifecycle_phase == Phase::Ready && current_next_sequence > 0 }
            update {}
            to Ready
            emit WorkIntentRecordedEventAuthorized {
                event_kind: MobCoordinationEventKind::WorkIntentRecorded,
                intent_id: intent_id,
                sequence: current_next_sequence,
                next_sequence: current_next_sequence + 1
            }
        }

        transition PublishWorkIntentStatusChangedEvent {
            on input PublishWorkIntentStatusChangedEvent { current_next_sequence, intent_id, status }
            guard { self.lifecycle_phase == Phase::Ready && current_next_sequence > 0 }
            update {}
            to Ready
            emit WorkIntentStatusChangedEventAuthorized {
                event_kind: MobCoordinationEventKind::WorkIntentStatusChanged,
                intent_id: intent_id,
                status: status,
                sequence: current_next_sequence,
                next_sequence: current_next_sequence + 1
            }
        }

        transition PublishResourceClaimRecordedEvent {
            on input PublishResourceClaimRecordedEvent { current_next_sequence, claim_id, kind }
            guard { self.lifecycle_phase == Phase::Ready && current_next_sequence > 0 }
            update {}
            to Ready
            emit ResourceClaimRecordedEventAuthorized {
                event_kind: MobCoordinationEventKind::ResourceClaimRecorded,
                claim_id: claim_id,
                kind: kind,
                sequence: current_next_sequence,
                next_sequence: current_next_sequence + 1
            }
        }

        transition PublishResourceClaimStatusChangedEvent {
            on input PublishResourceClaimStatusChangedEvent { current_next_sequence, claim_id, status }
            guard { self.lifecycle_phase == Phase::Ready && current_next_sequence > 0 }
            update {}
            to Ready
            emit ResourceClaimStatusChangedEventAuthorized {
                event_kind: MobCoordinationEventKind::ResourceClaimStatusChanged,
                claim_id: claim_id,
                status: status,
                sequence: current_next_sequence,
                next_sequence: current_next_sequence + 1
            }
        }

        transition PublishResourceClaimOverlapObservedEvent {
            on input PublishResourceClaimOverlapObservedEvent { current_next_sequence, claim_id, overlap_ids }
            guard { self.lifecycle_phase == Phase::Ready && current_next_sequence > 0 }
            update {}
            to Ready
            emit ResourceClaimOverlapObservedEventAuthorized {
                event_kind: MobCoordinationEventKind::ResourceClaimOverlapObserved,
                claim_id: claim_id,
                overlap_ids: overlap_ids,
                sequence: current_next_sequence,
                next_sequence: current_next_sequence + 1
            }
        }

        transition RecordWorkIntentPlanned {
            on input RecordWorkIntentStatus { requested_status }
            guard { self.lifecycle_phase == Phase::Ready && requested_status == MobCoordinationWorkIntentStatus::Planned }
            update {}
            to Ready
            emit WorkIntentStatusAuthorized { status: MobCoordinationWorkIntentStatus::Planned }
        }

        transition RecordWorkIntentActive {
            on input RecordWorkIntentStatus { requested_status }
            guard { self.lifecycle_phase == Phase::Ready && requested_status == MobCoordinationWorkIntentStatus::Active }
            update {}
            to Ready
            emit WorkIntentStatusAuthorized { status: MobCoordinationWorkIntentStatus::Active }
        }

        transition RecordWorkIntentBlocked {
            on input RecordWorkIntentStatus { requested_status }
            guard { self.lifecycle_phase == Phase::Ready && requested_status == MobCoordinationWorkIntentStatus::Blocked }
            update {}
            to Ready
            emit WorkIntentStatusAuthorized { status: MobCoordinationWorkIntentStatus::Blocked }
        }

        transition RecordWorkIntentCompleted {
            on input RecordWorkIntentStatus { requested_status }
            guard { self.lifecycle_phase == Phase::Ready && requested_status == MobCoordinationWorkIntentStatus::Completed }
            update {}
            to Ready
            emit WorkIntentStatusAuthorized { status: MobCoordinationWorkIntentStatus::Completed }
        }

        transition RecordWorkIntentCancelled {
            on input RecordWorkIntentStatus { requested_status }
            guard { self.lifecycle_phase == Phase::Ready && requested_status == MobCoordinationWorkIntentStatus::Cancelled }
            update {}
            to Ready
            emit WorkIntentStatusAuthorized { status: MobCoordinationWorkIntentStatus::Cancelled }
        }

        transition UpdateLiveWorkIntentPlanned {
            on input UpdateWorkIntentStatus { requested_status, is_expired }
            guard { self.lifecycle_phase == Phase::Ready && requested_status == MobCoordinationWorkIntentStatus::Planned && is_expired == false }
            update {}
            to Ready
            emit WorkIntentStatusAuthorized { status: MobCoordinationWorkIntentStatus::Planned }
        }

        transition UpdateLiveWorkIntentActive {
            on input UpdateWorkIntentStatus { requested_status, is_expired }
            guard { self.lifecycle_phase == Phase::Ready && requested_status == MobCoordinationWorkIntentStatus::Active && is_expired == false }
            update {}
            to Ready
            emit WorkIntentStatusAuthorized { status: MobCoordinationWorkIntentStatus::Active }
        }

        transition UpdateLiveWorkIntentBlocked {
            on input UpdateWorkIntentStatus { requested_status, is_expired }
            guard { self.lifecycle_phase == Phase::Ready && requested_status == MobCoordinationWorkIntentStatus::Blocked && is_expired == false }
            update {}
            to Ready
            emit WorkIntentStatusAuthorized { status: MobCoordinationWorkIntentStatus::Blocked }
        }

        transition UpdateWorkIntentCompleted {
            on input UpdateWorkIntentStatus { requested_status, is_expired }
            guard { self.lifecycle_phase == Phase::Ready && requested_status == MobCoordinationWorkIntentStatus::Completed }
            update {}
            to Ready
            emit WorkIntentStatusAuthorized { status: MobCoordinationWorkIntentStatus::Completed }
        }

        transition UpdateWorkIntentCancelled {
            on input UpdateWorkIntentStatus { requested_status, is_expired }
            guard { self.lifecycle_phase == Phase::Ready && requested_status == MobCoordinationWorkIntentStatus::Cancelled }
            update {}
            to Ready
            emit WorkIntentStatusAuthorized { status: MobCoordinationWorkIntentStatus::Cancelled }
        }

        transition RecordResourceClaimActive {
            on input RecordResourceClaimStatus { requested_status }
            guard { self.lifecycle_phase == Phase::Ready && requested_status == MobCoordinationResourceClaimStatus::Active }
            update {}
            to Ready
            emit ResourceClaimStatusAuthorized { status: MobCoordinationResourceClaimStatus::Active }
        }

        transition RecordResourceClaimReleased {
            on input RecordResourceClaimStatus { requested_status }
            guard { self.lifecycle_phase == Phase::Ready && requested_status == MobCoordinationResourceClaimStatus::Released }
            update {}
            to Ready
            emit ResourceClaimStatusAuthorized { status: MobCoordinationResourceClaimStatus::Released }
        }

        transition RecordResourceClaimExpired {
            on input RecordResourceClaimStatus { requested_status }
            guard { self.lifecycle_phase == Phase::Ready && requested_status == MobCoordinationResourceClaimStatus::Expired }
            update {}
            to Ready
            emit ResourceClaimStatusAuthorized { status: MobCoordinationResourceClaimStatus::Expired }
        }

        transition RecordResourceClaimCancelled {
            on input RecordResourceClaimStatus { requested_status }
            guard { self.lifecycle_phase == Phase::Ready && requested_status == MobCoordinationResourceClaimStatus::Cancelled }
            update {}
            to Ready
            emit ResourceClaimStatusAuthorized { status: MobCoordinationResourceClaimStatus::Cancelled }
        }

        transition UpdateLiveResourceClaimActive {
            on input UpdateResourceClaimStatus { requested_status, is_expired }
            guard { self.lifecycle_phase == Phase::Ready && requested_status == MobCoordinationResourceClaimStatus::Active && is_expired == false }
            update {}
            to Ready
            emit ResourceClaimStatusAuthorized { status: MobCoordinationResourceClaimStatus::Active }
        }

        transition UpdateResourceClaimReleased {
            on input UpdateResourceClaimStatus { requested_status, is_expired }
            guard { self.lifecycle_phase == Phase::Ready && requested_status == MobCoordinationResourceClaimStatus::Released }
            update {}
            to Ready
            emit ResourceClaimStatusAuthorized { status: MobCoordinationResourceClaimStatus::Released }
        }

        transition UpdateResourceClaimExpired {
            on input UpdateResourceClaimStatus { requested_status, is_expired }
            guard { self.lifecycle_phase == Phase::Ready && requested_status == MobCoordinationResourceClaimStatus::Expired }
            update {}
            to Ready
            emit ResourceClaimStatusAuthorized { status: MobCoordinationResourceClaimStatus::Expired }
        }

        transition UpdateResourceClaimCancelled {
            on input UpdateResourceClaimStatus { requested_status, is_expired }
            guard { self.lifecycle_phase == Phase::Ready && requested_status == MobCoordinationResourceClaimStatus::Cancelled }
            update {}
            to Ready
            emit ResourceClaimStatusAuthorized { status: MobCoordinationResourceClaimStatus::Cancelled }
        }
    }
}
