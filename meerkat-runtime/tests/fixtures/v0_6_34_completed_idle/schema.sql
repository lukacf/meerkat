CREATE TABLE sessions (
    session_id TEXT PRIMARY KEY,
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL,
    message_count INTEGER NOT NULL,
    total_tokens INTEGER NOT NULL,
    metadata_json TEXT NOT NULL,
    session_json BLOB NOT NULL
);
CREATE INDEX sessions_updated_idx
ON sessions(updated_at_ms DESC, session_id ASC);
CREATE TABLE runtime_input_states (
    runtime_id TEXT NOT NULL,
    input_id TEXT NOT NULL,
    state_json BLOB NOT NULL,
    PRIMARY KEY (runtime_id, input_id)
);
CREATE TABLE runtime_boundary_receipts (
    runtime_id TEXT NOT NULL,
    run_id TEXT NOT NULL,
    sequence INTEGER NOT NULL,
    receipt_json BLOB NOT NULL,
    PRIMARY KEY (runtime_id, run_id, sequence)
);
CREATE TABLE runtime_session_snapshots (
    runtime_id TEXT PRIMARY KEY,
    session_snapshot BLOB NOT NULL
);
CREATE TABLE runtime_states (
    runtime_id TEXT PRIMARY KEY,
    runtime_state_json BLOB NOT NULL
);
CREATE TABLE runtime_ops_lifecycle (
    runtime_id TEXT PRIMARY KEY,
    state_json BLOB NOT NULL
);
CREATE TABLE runtime_auth_oauth_flow_state (
    id TEXT PRIMARY KEY,
    state_json BLOB NOT NULL
);
