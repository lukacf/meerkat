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
            session_envelope_released_v3: u64,
            stored_input_state_version: u64,
            // The version a stored input row is written with when it carries
            // no fact only the current version can read. A requeue-across-stop
            // fact (a stop's requeue mark or the requeue-expired abandon
            // reason) needs the current version; every other row stays at the
            // baseline, so a host that never requeues keeps the older
            // reader's downgrade.
            stored_input_state_baseline_version: u64,
            stored_input_state_migration_v6: u64,
            stored_input_state_migration_v5: u64,
            stored_input_state_migration_v4: u64,
            stored_input_state_released_v3: u64,
            session_metadata_schema_version: u64,
        }

        init(Ready) {
            session_envelope_version = 4,
            session_envelope_released_v3 = 3,
            stored_input_state_version = 8,
            stored_input_state_baseline_version = 7,
            stored_input_state_migration_v6 = 6,
            stored_input_state_migration_v5 = 5,
            stored_input_state_migration_v4 = 4,
            stored_input_state_released_v3 = 3,
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

        transition MigrateReleasedSessionEnvelopeV3ToV4 {
            on input RestoreSessionEnvelopeVersion { persisted_version }
            guard {
                self.lifecycle_phase == Phase::Ready
                && persisted_version == self.session_envelope_released_v3
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

        transition RestoreBaselineStoredInputStateVersion {
            on input RestoreStoredInputStateVersion { persisted_version }
            guard {
                self.lifecycle_phase == Phase::Ready
                && persisted_version == self.stored_input_state_baseline_version
            }
            update {}
            to Ready
            emit VersionRestoreAuthorized {
                field: SessionPersistenceVersionField::StoredInputState,
                version: self.stored_input_state_version
            }
        }

        transition MigrateStoredInputStateV6ToV7 {
            on input RestoreStoredInputStateVersion { persisted_version }
            guard {
                self.lifecycle_phase == Phase::Ready
                && persisted_version == self.stored_input_state_migration_v6
            }
            update {}
            to Ready
            emit VersionRestoreAuthorized {
                field: SessionPersistenceVersionField::StoredInputState,
                version: self.stored_input_state_version
            }
        }

        transition MigrateStoredInputStateV5ToV6 {
            on input RestoreStoredInputStateVersion { persisted_version }
            guard {
                self.lifecycle_phase == Phase::Ready
                && persisted_version == self.stored_input_state_migration_v5
            }
            update {}
            to Ready
            emit VersionRestoreAuthorized {
                field: SessionPersistenceVersionField::StoredInputState,
                version: self.stored_input_state_version
            }
        }

        transition MigrateStoredInputStateV4ToV5 {
            on input RestoreStoredInputStateVersion { persisted_version }
            guard {
                self.lifecycle_phase == Phase::Ready
                && persisted_version == self.stored_input_state_migration_v4
            }
            update {}
            to Ready
            emit VersionRestoreAuthorized {
                field: SessionPersistenceVersionField::StoredInputState,
                version: self.stored_input_state_version
            }
        }

        transition MigrateReleasedStoredInputStateV3ToV5 {
            on input RestoreStoredInputStateVersion { persisted_version }
            guard {
                self.lifecycle_phase == Phase::Ready
                && persisted_version == self.stored_input_state_released_v3
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
