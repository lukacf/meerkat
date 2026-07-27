use meerkat_machine_dsl::machine;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum SessionPersistenceVersionField {
    #[default]
    SessionEnvelope,
    StoredInputState,
    SessionMetadataSchema,
    TranscriptHistoryWitnessFormat,
}

machine! {
    machine SessionPersistenceVersionAuthorityMachine {
        version: 1,
        rust: "self" / "catalog::dsl::session_persistence_version_authority",

        state {
            lifecycle_phase: SessionPersistenceVersionAuthorityPhase,
            session_envelope_version: u64,
            stored_input_state_version: u64,
            stored_input_state_migration_v3: u64,
            session_metadata_schema_version: u64,
            transcript_history_witness_format: u64,
            transcript_history_witness_format_v2: u64,
        }

        init(Ready) {
            session_envelope_version = 2,
            stored_input_state_version = 4,
            stored_input_state_migration_v3 = 3,
            session_metadata_schema_version = 2,
            transcript_history_witness_format = 3,
            transcript_history_witness_format_v2 = 2,
        }

        terminal []

        phase SessionPersistenceVersionAuthorityPhase {
            Ready,
        }

        input SessionPersistenceVersionAuthorityInput {
            RestoreSessionEnvelopeVersion { persisted_version: u64 },
            RestoreStoredInputStateVersion { persisted_version: u64 },
            RestoreSessionMetadataSchemaVersion { persisted_version: u64 },
            RestoreTranscriptHistoryWitnessFormat { persisted_version: u64 },
        }

        effect SessionPersistenceVersionAuthorityEffect {
            VersionRestoreAuthorized {
                field: Enum<SessionPersistenceVersionField>,
                version: u64,
            },
        }

        disposition VersionRestoreAuthorized => local seam NoOwnerRealization,

        transition RestoreCurrentSessionEnvelopeVersion {
            on input RestoreSessionEnvelopeVersion { persisted_version }
            guard {
                self.lifecycle_phase == Phase::Ready
                && persisted_version == self.session_envelope_version
            }
            update {}
            to Ready
            emit VersionRestoreAuthorized {
                field: SessionPersistenceVersionField::SessionEnvelope,
                version: self.session_envelope_version
            }
        }

        transition RestoreCurrentStoredInputStateVersion {
            on input RestoreStoredInputStateVersion { persisted_version }
            guard {
                self.lifecycle_phase == Phase::Ready
                && persisted_version == self.stored_input_state_version
            }
            update {}
            to Ready
            emit VersionRestoreAuthorized {
                field: SessionPersistenceVersionField::StoredInputState,
                version: self.stored_input_state_version
            }
        }

        transition MigrateStoredInputStateV3ToV4 {
            on input RestoreStoredInputStateVersion { persisted_version }
            guard {
                self.lifecycle_phase == Phase::Ready
                && persisted_version == self.stored_input_state_migration_v3
            }
            update {}
            to Ready
            emit VersionRestoreAuthorized {
                field: SessionPersistenceVersionField::StoredInputState,
                version: self.stored_input_state_version
            }
        }

        transition RestoreCurrentSessionMetadataSchemaVersion {
            on input RestoreSessionMetadataSchemaVersion { persisted_version }
            guard {
                self.lifecycle_phase == Phase::Ready
                && persisted_version == self.session_metadata_schema_version
            }
            update {}
            to Ready
            emit VersionRestoreAuthorized {
                field: SessionPersistenceVersionField::SessionMetadataSchema,
                version: self.session_metadata_schema_version
            }
        }

        // Transcript-history witness format axis. v2 evidence is ACCEPTED
        // indefinitely (mixed v2/v3 stores, per-session lazy upgrade); the
        // generated restore authorizer is a membership gate over {2, 3} —
        // the typed carrier keeps the observed format, verification runs
        // under the format the evidence declares, and only unknown formats
        // refuse. The emitted version names the current mint format.
        transition RestoreCurrentTranscriptHistoryWitnessFormat {
            on input RestoreTranscriptHistoryWitnessFormat { persisted_version }
            guard {
                self.lifecycle_phase == Phase::Ready
                && persisted_version == self.transcript_history_witness_format
            }
            update {}
            to Ready
            emit VersionRestoreAuthorized {
                field: SessionPersistenceVersionField::TranscriptHistoryWitnessFormat,
                version: self.transcript_history_witness_format
            }
        }

        transition AcceptTranscriptHistoryWitnessFormatV2 {
            on input RestoreTranscriptHistoryWitnessFormat { persisted_version }
            guard {
                self.lifecycle_phase == Phase::Ready
                && persisted_version == self.transcript_history_witness_format_v2
            }
            update {}
            to Ready
            emit VersionRestoreAuthorized {
                field: SessionPersistenceVersionField::TranscriptHistoryWitnessFormat,
                version: self.transcript_history_witness_format
            }
        }
    }
}
