CREATE TABLE IF NOT EXISTS egress_outputs (
                    output_id TEXT PRIMARY KEY,
                    root TEXT NOT NULL,
                    issuer_id TEXT NOT NULL,
                    logical_key TEXT NOT NULL,
                    request_digest TEXT NOT NULL,
                    sender_id TEXT NOT NULL,
                    recipient TEXT NOT NULL,
                    part_count INTEGER NOT NULL,
                    incident_delivery INTEGER NOT NULL,
                    created_at_ms INTEGER NOT NULL,
                    delivery_class TEXT NOT NULL DEFAULT 'governed',
                    caller_identity TEXT,
                    recipient_member TEXT,
                    UNIQUE (issuer_id, logical_key)
                );
                CREATE TABLE IF NOT EXISTS egress_parts (
                    output_id TEXT NOT NULL,
                    part_index INTEGER NOT NULL,
                    state TEXT NOT NULL,
                    text TEXT,
                    caption TEXT,
                    mime_type TEXT,
                    media_ref TEXT,
                    entities TEXT,
                    caption_entities TEXT,
                    attempts INTEGER NOT NULL DEFAULT 0,
                    unknown_attempts INTEGER NOT NULL DEFAULT 0,
                    provider_msg_id TEXT,
                    error TEXT,
                    PRIMARY KEY (output_id, part_index),
                    FOREIGN KEY (output_id) REFERENCES egress_outputs(output_id)
                );
                -- In-band migrations (IF NOT EXISTS runs against existing
                -- production stores too): the stale sweep and drainer scan by
                -- part state and output age on every pass; without these the
                -- scans are O(all parts ever sent) per pass, forever.
                CREATE INDEX IF NOT EXISTS idx_egress_parts_state
                    ON egress_parts(state);
                CREATE INDEX IF NOT EXISTS idx_egress_outputs_created
                    ON egress_outputs(created_at_ms);

ALTER TABLE egress_outputs ADD COLUMN choice_ref TEXT;

CREATE INDEX IF NOT EXISTS idx_egress_outputs_choice_ref ON egress_outputs(choice_ref);

CREATE TABLE IF NOT EXISTS egress_choice_taps (
                    output_id TEXT PRIMARY KEY,
                    channel_id TEXT NOT NULL,
                    source_event_id TEXT NOT NULL,
                    callback_query_id TEXT,
                    value TEXT NOT NULL,
                    tapped_at_ms INTEGER NOT NULL,
                    FOREIGN KEY (output_id) REFERENCES egress_outputs(output_id)
                );

ALTER TABLE egress_parts ADD COLUMN choices TEXT;
