-- Frozen from Core 74e62b9c7c46b2b8fc0b3b8ace576fd3f9a28838.
-- CREATE_SCHEMA_SQL plus the event-route index installed by migration 2.
-- Migration 3 is a no-op on this already fenced request table.

CREATE TABLE IF NOT EXISTS mob_events (
    cursor INTEGER PRIMARY KEY,
    mob_id TEXT,
    event_json BLOB NOT NULL
);
CREATE TABLE IF NOT EXISTS mob_event_meta (
    key TEXT PRIMARY KEY,
    value INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS mob_external_deliveries (
    mob_id TEXT NOT NULL,
    idempotency_key TEXT NOT NULL,
    record_json BLOB NOT NULL,
    PRIMARY KEY (mob_id, idempotency_key)
);
CREATE TABLE IF NOT EXISTS mob_runs (
    run_id TEXT PRIMARY KEY,
    run_json BLOB NOT NULL
);
CREATE TABLE IF NOT EXISTS mob_run_remote_turn_intents (
    run_id TEXT NOT NULL,
    dispatch_sequence BLOB NOT NULL,
    intent_json BLOB NOT NULL,
    PRIMARY KEY (run_id, dispatch_sequence)
);
CREATE TABLE IF NOT EXISTS mob_run_remote_turn_receipts (
    run_id TEXT NOT NULL,
    dispatch_sequence BLOB NOT NULL,
    receipt_json BLOB NOT NULL,
    PRIMARY KEY (run_id, dispatch_sequence)
);
CREATE TABLE IF NOT EXISTS mob_run_observers (
    run_id TEXT PRIMARY KEY,
    binding_json BLOB NOT NULL
);
CREATE TABLE IF NOT EXISTS mob_specs (
    mob_id TEXT PRIMARY KEY,
    spec_json BLOB NOT NULL
);
CREATE TABLE IF NOT EXISTS mob_runtime_supervisors (
    mob_id TEXT PRIMARY KEY,
    record_json BLOB NOT NULL
);
CREATE TABLE IF NOT EXISTS mob_runtime_host_authorities (
    mob_id TEXT NOT NULL,
    host_id TEXT NOT NULL,
    record_json BLOB NOT NULL,
    PRIMARY KEY (mob_id, host_id)
);
CREATE TABLE IF NOT EXISTS mob_runtime_host_binding_generation_highwaters (
    mob_id TEXT NOT NULL,
    host_id TEXT NOT NULL,
    binding_generation BLOB NOT NULL,
    PRIMARY KEY (mob_id, host_id)
);
CREATE TABLE IF NOT EXISTS mob_runtime_member_operator_requests (
    mob_id TEXT NOT NULL,
    agent_identity TEXT NOT NULL,
    generation BLOB NOT NULL,
    fence_token BLOB NOT NULL,
    host_id TEXT NOT NULL,
    binding_generation BLOB NOT NULL,
    member_session_id TEXT NOT NULL,
    request_id TEXT NOT NULL,
    record_json BLOB NOT NULL,
    PRIMARY KEY (mob_id, agent_identity, generation, fence_token, host_id, binding_generation, member_session_id, request_id)
);
CREATE TABLE IF NOT EXISTS mob_runtime_placed_spawns (
    mob_id TEXT NOT NULL,
    agent_identity TEXT NOT NULL,
    record_json BLOB NOT NULL,
    PRIMARY KEY (mob_id, agent_identity)
);
CREATE TABLE IF NOT EXISTS mob_runtime_operator_grants (
    mob_id TEXT NOT NULL,
    principal TEXT NOT NULL,
    record_json BLOB NOT NULL,
    PRIMARY KEY (mob_id, principal)
);
CREATE TABLE IF NOT EXISTS mob_runtime_member_event_cursors (
    mob_id TEXT NOT NULL,
    agent_identity TEXT NOT NULL,
    record_json BLOB NOT NULL,
    PRIMARY KEY (mob_id, agent_identity)
);
CREATE TABLE IF NOT EXISTS mob_runtime_member_live_cleanups (
    mob_id TEXT NOT NULL,
    cleanup_id TEXT NOT NULL,
    record_json BLOB NOT NULL,
    PRIMARY KEY (mob_id, cleanup_id)
);
CREATE TABLE IF NOT EXISTS mob_runtime_binding_overlays (
    mob_id TEXT NOT NULL,
    agent_identity TEXT NOT NULL,
    generation INTEGER NOT NULL,
    record_json BLOB NOT NULL,
    PRIMARY KEY (mob_id, agent_identity, generation)
);
CREATE TABLE IF NOT EXISTS mob_identity_store_meta (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS mob_identity_intents (
    mob_id TEXT NOT NULL,
    agent_identity TEXT NOT NULL,
    declaration_scope TEXT,
    session_id TEXT,
    lineage_id TEXT,
    record_json BLOB NOT NULL,
    PRIMARY KEY (mob_id, agent_identity)
);
CREATE UNIQUE INDEX IF NOT EXISTS mob_identity_intents_session
    ON mob_identity_intents (session_id)
    WHERE session_id IS NOT NULL;
CREATE UNIQUE INDEX IF NOT EXISTS mob_identity_intents_lineage
    ON mob_identity_intents (lineage_id)
    WHERE lineage_id IS NOT NULL;
CREATE TABLE IF NOT EXISTS mob_identity_leases (
    mob_id TEXT NOT NULL,
    agent_identity TEXT NOT NULL,
    epoch_highwater BLOB NOT NULL,
    active_holder_id TEXT,
    active_incarnation_id TEXT,
    active_epoch BLOB,
    active_renewed_at_ms BLOB,
    active_expires_at_ms BLOB,
    authority_digest TEXT NOT NULL,
    record_json BLOB NOT NULL,
    PRIMARY KEY (mob_id, agent_identity)
);
CREATE TABLE IF NOT EXISTS mob_identity_operation_receipts (
    mob_id TEXT NOT NULL,
    subject_kind TEXT NOT NULL,
    subject_id TEXT NOT NULL,
    slot_kind TEXT NOT NULL,
    slot_key_version INTEGER NOT NULL,
    slot_digest TEXT NOT NULL,
    subject_json BLOB NOT NULL,
    slot_json BLOB NOT NULL,
    subject_identity TEXT,
    effect_kind TEXT NOT NULL,
    receipt_json BLOB NOT NULL,
    PRIMARY KEY (
        mob_id, subject_kind, subject_id, slot_kind,
        slot_key_version, slot_digest
    )
);
CREATE INDEX IF NOT EXISTS mob_identity_receipts_identity
    ON mob_identity_operation_receipts (mob_id, subject_identity);
CREATE TABLE IF NOT EXISTS mob_identity_intent_mutation_receipts (
    mob_id TEXT NOT NULL,
    request_id TEXT NOT NULL,
    agent_identity TEXT NOT NULL,
    request_digest TEXT NOT NULL,
    receipt_json BLOB NOT NULL,
    PRIMARY KEY (mob_id, request_id)
);
CREATE TABLE IF NOT EXISTS mob_identity_convergence_resolution_receipts (
    mob_id TEXT NOT NULL,
    request_id TEXT NOT NULL,
    agent_identity TEXT NOT NULL,
    request_digest TEXT NOT NULL,
    receipt_json BLOB NOT NULL,
    PRIMARY KEY (mob_id, request_id)
);
CREATE TABLE IF NOT EXISTS mob_identity_adoption_receipts (
    mob_id TEXT NOT NULL,
    request_id TEXT NOT NULL,
    agent_identity TEXT NOT NULL,
    request_digest TEXT NOT NULL,
    receipt_json BLOB NOT NULL,
    PRIMARY KEY (mob_id, request_id)
);
CREATE TABLE IF NOT EXISTS mob_identity_statuses (
    mob_id TEXT NOT NULL,
    agent_identity TEXT NOT NULL,
    status_json BLOB NOT NULL,
    PRIMARY KEY (mob_id, agent_identity)
);
CREATE TABLE IF NOT EXISTS realm_profiles (
    name TEXT PRIMARY KEY,
    profile_json BLOB NOT NULL,
    revision INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS mob_forked_participants (
    capability_id TEXT PRIMARY KEY,
    request_id TEXT NOT NULL UNIQUE,
    record_json BLOB NOT NULL,
    revision INTEGER NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE UNIQUE INDEX mob_forked_participants_fork_session
    ON mob_forked_participants (json_extract(record_json, '$.planned_child_session_id'));
CREATE TABLE IF NOT EXISTS mob_forked_participant_member_associations (
    mob_id TEXT NOT NULL,
    association_key TEXT NOT NULL,
    record_json BLOB NOT NULL,
    PRIMARY KEY (mob_id, association_key)
);
CREATE TABLE IF NOT EXISTS mob_temporary_councils (
    council_id TEXT PRIMARY KEY,
    temporary_mob_id TEXT NOT NULL UNIQUE,
    record_json BLOB NOT NULL,
    revision INTEGER NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS mob_runtime_callback_claim_verifiers (
    mob_id TEXT PRIMARY KEY,
    verifier BLOB NOT NULL
);
CREATE TABLE IF NOT EXISTS mob_runtime_incarnations (
    mob_id TEXT PRIMARY KEY,
    incarnation TEXT NOT NULL,
    origin TEXT NOT NULL CHECK (origin IN ('created', 'migrated')),
    provenance TEXT NOT NULL,
    finalized_at_ms INTEGER
)
CREATE INDEX IF NOT EXISTS mob_events_mob_cursor ON mob_events (mob_id, cursor);
CREATE TABLE meerkat_schema (domain TEXT PRIMARY KEY, version INTEGER NOT NULL);
INSERT INTO meerkat_schema VALUES ('mob', 13);
