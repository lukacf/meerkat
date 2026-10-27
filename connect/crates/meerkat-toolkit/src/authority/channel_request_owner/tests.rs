use super::*;

#[test]
fn reader_never_creates_a_database_or_migrates_legacy_root_rows() {
    let directory = tempfile::tempdir().unwrap();
    let missing = directory.path().join("missing.sqlite");
    assert!(matches!(
        SqliteChannelSourceReader::open(&missing),
        Err(ChannelSourceError::Unavailable)
    ));
    assert!(!missing.exists());
    let path = directory.path().join("source.sqlite");
    let connection = Connection::open(&path).unwrap();
    connection.execute_batch("CREATE TABLE channel_dispatch_groups(channel_id TEXT, head_source_event_id TEXT, native_request_root TEXT); INSERT INTO channel_dispatch_groups VALUES ('channel','head','{\"version\":2,\"claims\":{},\"signature\":\"old\"}');").unwrap();
    let before: String = connection
        .query_row(
            "SELECT native_request_root FROM channel_dispatch_groups",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let reader = SqliteChannelSourceReader::open(&path).unwrap();
    assert_eq!(
        reader
            .observe_exact(&ChannelRequestKey::new("channel", "head").unwrap())
            .unwrap_err(),
        ChannelSourceError::MigrationRequired
    );
    let after: String = connection
        .query_row(
            "SELECT native_request_root FROM channel_dispatch_groups",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(before, after);
    assert_eq!(
        columns(&connection, "channel_dispatch_groups").unwrap(),
        ["channel_id", "head_source_event_id", "native_request_root"]
    );
}

#[test]
fn frozen_source_object_rejects_duplicate_keys_without_normalizing_numbers() {
    assert!(frozen_envelope(r#"{"sender_id":"a","sender_id":"b"}"#).is_err());
    let source = r#"{"sender_id":"a","value":1.00}"#;
    assert_eq!(frozen_envelope(source).unwrap()["value"].get(), "1.00");
}

// The existing application inbox schema and exact frozen envelopes are used
// directly. No signer, policy permission, native admission or custody fixture.
struct SourceFixture {
    _directory: tempfile::TempDir,
    path: PathBuf,
    key: ChannelRequestKey,
    provenance: String,
    members: Vec<ChannelSourceMember>,
}
impl SourceFixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("channels.sqlite3");
        let connection = Connection::open(&path).unwrap();
        connection.execute_batch("CREATE TABLE channel_inbox(channel_id TEXT,source_event_id TEXT,state TEXT,envelope TEXT,source_provenance TEXT,dispatch_continuity_version INTEGER,PRIMARY KEY(channel_id,source_event_id));
            CREATE TABLE channel_dispatch_groups(channel_id TEXT,head_source_event_id TEXT,identity TEXT,member_count INTEGER,payload TEXT,source_provenance TEXT,PRIMARY KEY(channel_id,head_source_event_id));
            CREATE TABLE channel_dispatch_members(channel_id TEXT,source_event_id TEXT,head_source_event_id TEXT,ordinal INTEGER,envelope_digest TEXT,PRIMARY KEY(channel_id,source_event_id));").unwrap();
        let source = ChannelSourceProvenance {
            version: 1,
            channel_id: "slack".into(),
            extension_id: "slack-extension".into(),
            service_subject: "service:slack".into(),
            provider_account: "account:family".into(),
            requester: "person:original".into(),
            sender_id: "sender".into(),
            partition: r#"{"group_id":1.00,"recipient_or_chat_id":1000.0,"thread_id":null}"#.into(),
            policy_digest: "policy-1".into(),
        };
        let provenance = String::from_utf8(canonical_json(&source).unwrap()).unwrap();
        let mut members = Vec::new();
        for (ordinal, id) in ["head", "tail"].into_iter().enumerate() {
            let envelope = format!(
                r#"{{"group_id":1.00,"recipient_or_chat_id":1000.0,"sender_id":"sender","text":"{id}","thread_id":null}}"#
            );
            let digest = hex::encode(Sha256::digest(envelope.as_bytes()));
            connection
                .execute(
                    "INSERT INTO channel_inbox VALUES ('slack',?,'accepted',?,?,1)",
                    params![id, envelope, provenance],
                )
                .unwrap();
            connection
                .execute(
                    "INSERT INTO channel_dispatch_members VALUES ('slack',?,'head',?,?)",
                    params![id, ordinal, digest],
                )
                .unwrap();
            members.push(ChannelSourceMember {
                source_event_id: id.into(),
                envelope_sha256: digest,
            });
        }
        connection.execute("INSERT INTO channel_dispatch_groups VALUES ('slack','head','agent:target',2,NULL,?)", [&provenance]).unwrap();
        Self {
            _directory: directory,
            path,
            key: ChannelRequestKey::new("slack", "head").unwrap(),
            provenance,
            members,
        }
    }
    fn observe(&self) -> Result<ChannelInboxObservation> {
        SqliteChannelSourceReader::open(&self.path)?.observe_exact(&self.key)
    }
    fn frozen_rows(&self) -> Vec<(String, String, String, String)> {
        let connection =
            Connection::open_with_flags(&self.path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        let mut statement = connection.prepare("SELECT source_event_id,state,envelope,source_provenance FROM channel_inbox ORDER BY source_event_id").unwrap();
        statement
            .query_map([], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })
            .unwrap()
            .collect::<std::result::Result<Vec<_>, _>>()
            .unwrap()
    }
}

#[test]
fn sqlite_observation_preserves_order_and_raw_numeric_partition_without_source_mutation() {
    let fixture = SourceFixture::new();
    let before = fixture.frozen_rows();
    let observed = fixture.observe().unwrap();
    assert_eq!(observed.key, fixture.key);
    assert_eq!(observed.target_identity, "agent:target");
    assert_eq!(observed.members, fixture.members);
    assert_eq!(
        observed.source.partition,
        r#"{"group_id":1.00,"recipient_or_chat_id":1000.0,"thread_id":null}"#
    );
    assert_eq!(
        canonical_json(&observed.source).unwrap(),
        fixture.provenance.as_bytes()
    );
    assert_eq!(fixture.frozen_rows(), before);
    assert_eq!(fixture.observe().unwrap(), observed);
    let connection =
        Connection::open_with_flags(&fixture.path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    assert!(
        !columns(&connection, "channel_dispatch_groups")
            .unwrap()
            .iter()
            .any(|name| name == "native_request_root")
    );
}

#[test]
fn sqlite_observation_refuses_changed_digest_order_provenance_and_partition() {
    for changed in ["digest", "ordinal", "provenance", "partition"] {
        let fixture = SourceFixture::new();
        let connection = Connection::open(&fixture.path).unwrap();
        match changed {
            "digest" => {
                connection.execute("UPDATE channel_dispatch_members SET envelope_digest=? WHERE source_event_id='tail'", ["0".repeat(64)]).unwrap();
            }
            "ordinal" => {
                connection.execute("UPDATE channel_dispatch_members SET ordinal=2 WHERE source_event_id='tail'", []).unwrap();
            }
            "provenance" => {
                connection.execute("UPDATE channel_inbox SET source_provenance='{}' WHERE source_event_id='tail'", []).unwrap();
            }
            "partition" => {
                // Preserve valid canonical provenance and agreement across all
                // rows, but change raw numeric spelling compared with each
                // still correctly hashed original envelope.
                let mut source: ChannelSourceProvenance =
                    serde_json::from_str(&fixture.provenance).unwrap();
                source.partition =
                    r#"{"group_id":1.0,"recipient_or_chat_id":1000.0,"thread_id":null}"#.into();
                let encoded = String::from_utf8(canonical_json(&source).unwrap()).unwrap();
                connection
                    .execute(
                        "UPDATE channel_dispatch_groups SET source_provenance=?",
                        [&encoded],
                    )
                    .unwrap();
                connection
                    .execute("UPDATE channel_inbox SET source_provenance=?", [&encoded])
                    .unwrap();
            }
            _ => unreachable!(),
        }
        let before = fixture.frozen_rows();
        assert_eq!(
            fixture.observe().unwrap_err(),
            ChannelSourceError::InvalidRecord,
            "{changed}"
        );
        assert_eq!(fixture.frozen_rows(), before, "{changed}");
    }
}
