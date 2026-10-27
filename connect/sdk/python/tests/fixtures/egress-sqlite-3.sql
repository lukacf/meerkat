-- Generated from exact prior Toolkit owner 5fac4c12983d374c87574a06c1af8d91cfa30f15.
BEGIN TRANSACTION;
CREATE TABLE egress_format (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    namespace TEXT NOT NULL, owner TEXT NOT NULL, format TEXT NOT NULL,
    operation TEXT NOT NULL, source_state TEXT NOT NULL,
    source_observation_digest TEXT NOT NULL, schema_digest TEXT NOT NULL,
    request_binding TEXT NOT NULL
);
INSERT INTO "egress_format" VALUES(1,'channels/egress','frontend/egress','egress-sqlite-3','adopt-egress-sqlite-3','absent_database','a9da1d07d104df3f20788eba2c26446fb8338251221333cbd704dbad9d86328b','612d8c7851764250668008a5ffb9679a097b771bb899e9b18e2558b54ad0c015','{"migration_artifact_digest":"3333333333333333333333333333333333333333333333333333333333333333","owner_artifact_digest":"2222222222222222222222222222222222222222222222222222222222222222","previous_manifest_digest":null,"target_manifest_digest":"1111111111111111111111111111111111111111111111111111111111111111"}');
CREATE TABLE egress_outputs (
                    output_id TEXT PRIMARY KEY,
                    root TEXT NOT NULL,
                    issuer_id TEXT NOT NULL,
                    logical_key TEXT NOT NULL,
                    request_digest TEXT NOT NULL,
                    sender_id TEXT NOT NULL,
                    recipient TEXT NOT NULL,
                    part_count INTEGER NOT NULL,
                    incident_delivery INTEGER NOT NULL,
                    created_at_ms INTEGER NOT NULL, cancel_requested INTEGER NOT NULL DEFAULT 0, thread_id TEXT,
                    UNIQUE (issuer_id, logical_key)
                );
INSERT INTO "egress_outputs" VALUES('19782917-fc07-4142-8599-077dc19e2337','hc-notify-v1:host/person/prior-request','host/person','prior-request','ae3b087411d78f3f77674e4e316ff92f7f199c4fbf62d1ca363ad237978ee51b','s','chat',5,0,1000,0,'47');
CREATE TABLE egress_parts (
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
                    last_outcome TEXT,
                    provider_msg_id TEXT,
                    error TEXT,
                    PRIMARY KEY (output_id, part_index),
                    FOREIGN KEY (output_id) REFERENCES egress_outputs(output_id)
                );
INSERT INTO "egress_parts" VALUES('19782917-fc07-4142-8599-077dc19e2337',0,'sent','accepted',NULL,NULL,NULL,NULL,NULL,1,0,'provider_accepted','accepted-1',NULL);
INSERT INTO "egress_parts" VALUES('19782917-fc07-4142-8599-077dc19e2337',1,'unknown','uncertain',NULL,NULL,NULL,NULL,NULL,1,1,'unknown','uncertain-1','retained uncertainty');
INSERT INTO "egress_parts" VALUES('19782917-fc07-4142-8599-077dc19e2337',2,'pending','open',NULL,NULL,NULL,NULL,NULL,1,1,'unknown',NULL,NULL);
INSERT INTO "egress_parts" VALUES('19782917-fc07-4142-8599-077dc19e2337',3,'pending','never entered',NULL,NULL,NULL,NULL,NULL,0,0,NULL,NULL,NULL);
INSERT INTO "egress_parts" VALUES('19782917-fc07-4142-8599-077dc19e2337',4,'pending','refused',NULL,NULL,NULL,NULL,NULL,1,0,'known_not_sent',NULL,'retained refusal');
CREATE INDEX idx_egress_parts_state ON egress_parts(state);
CREATE INDEX idx_egress_outputs_created ON egress_outputs(created_at_ms);
COMMIT;
