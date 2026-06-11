use meerkat_machine_dsl::machine;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum SessionPersistenceVersionField {
    #[default]
    SessionEnvelope,
    StoredInputState,
    SessionMetadataSchema,
}

machine! {
    machine SessionPersistenceVersionAuthorityMachine {
        version: 1,
        rust: "self" / "catalog::dsl::session_persistence_version_authority",

        state {
            lifecycle_phase: SessionPersistenceVersionAuthorityPhase,
            session_envelope_version: u64,
            stored_input_state_version: u64,
            session_metadata_schema_version: u64,
        }

        init(Ready) {
            session_envelope_version = 2,
            stored_input_state_version = 3,
            session_metadata_schema_version = 2,
        }

        terminal []

        phase SessionPersistenceVersionAuthorityPhase {
            Ready,
        }

        input SessionPersistenceVersionAuthorityInput {
            RestoreSessionEnvelopeVersion { persisted_version: u64 },
            RestoreStoredInputStateVersion { persisted_version: u64 },
            RestoreSessionMetadataSchemaVersion { persisted_version: u64 },
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
    }
}
