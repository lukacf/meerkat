use super::*;

fn revision_frame(key: &str, witness: &str) -> NewConsoleFrame {
    let identity = ConsoleIdentityRecord {
        identity: "revision-worker".into(),
        display_name: "Revision worker".into(),
        runtime_key: "revision-runtime".into(),
        runtime_member_id: "revision-worker".into(),
        session_id: Some("revision-session".into()),
        member_id: None,
        visibility: ConsoleVisibility::Addressable,
        addressable: true,
        health: "ready".into(),
        topology_peers: Vec::new(),
        labels: BTreeMap::from([("witness".into(), witness.into())]),
    };
    let provenance = ConsoleFrameMemberProvenance {
        member: ConsoleMember {
            agent_identity: identity.runtime_member_id.clone(),
            role: "worker".into(),
            state: "running".into(),
            model_capabilities: Default::default(),
            runtime_mode: None,
            session_id: identity.session_id.clone(),
            wired_to: Vec::new(),
            labels: identity.labels.clone(),
            progress: None,
        },
        primary_mob_id: "revision-mob".into(),
        source_mob_id: "revision-mob".into(),
        lineage: Vec::new(),
        identity,
    };
    NewConsoleFrame {
        id: None,
        dedupe_key: key.into(),
        timestamp_ms: 1,
        runtime_key: "revision-runtime".into(),
        identity: "revision-worker".into(),
        conversation_id: Some("revision-worker".into()),
        session_id: Some("revision-session".into()),
        kind: "text_complete".into(),
        status: ConsoleFrameStatus::Completed,
        payload: json!({ "text": key }),
        source: ConsoleFrameSource {
            member_provenance: Some(provenance),
            kind: ConsoleFrameSourceKind::SessionHistory,
            source_cursor: None,
        },
        source_event_id: None,
        interaction_id: None,
        turn_id: None,
        run_id: None,
        parent_frame_id: None,
        caused_by_frame_id: None,
    }
}

async fn revision(store: &dyn ConsoleLogStore) -> ConsoleLogResult<String> {
    store
        .history_prefix_revision()
        .await?
        .ok_or_else(|| std::io::Error::other("store did not provide a prefix revision").into())
}

async fn assert_local_revision_contract(store: &dyn ConsoleLogStore) -> ConsoleLogResult<()> {
    let initial_revision = revision(store).await?;
    let first = store
        .append_if_absent(revision_frame("first", "original"))
        .await?
        .frame;
    assert_eq!(revision(store).await?, initial_revision);
    let tail = store
        .append_if_absent(revision_frame("tail", "unchanged"))
        .await?
        .frame;
    assert_eq!(revision(store).await?, initial_revision);

    let updated = store
        .update_frame_status(&first.id, ConsoleFrameStatus::DeliveryFailed)
        .await?
        .ok_or("status update did not find the stored first frame")?;
    assert_eq!(updated.status, ConsoleFrameStatus::DeliveryFailed);
    assert_eq!(updated.cursor, first.cursor);
    assert_eq!(
        updated.source.member_provenance,
        first.source.member_provenance
    );
    assert_eq!(revision(store).await?, initial_revision);
    assert_eq!(store.latest_cursor().await?, Some(tail.cursor.clone()));

    store.clear_frames().await?;
    let cleared_revision = revision(store).await?;
    assert_ne!(cleared_revision, initial_revision);
    assert_eq!(store.latest_cursor().await?, None);
    let replacement = store
        .append_if_absent(revision_frame("first", "replacement"))
        .await?
        .frame;
    let restored_tail = store
        .append_if_absent(revision_frame("tail", "unchanged"))
        .await?
        .frame;

    // An identical final row and cursor cannot prove an unchanged prefix.
    assert_eq!(replacement.id, first.id);
    assert_eq!(replacement.cursor, first.cursor);
    assert_ne!(
        replacement.source.member_provenance,
        first.source.member_provenance
    );
    assert_eq!(restored_tail, tail);
    assert_eq!(store.latest_cursor().await?, Some(tail.cursor));
    assert_eq!(revision(store).await?, cleared_revision);
    assert_ne!(revision(store).await?, initial_revision);
    Ok(())
}

#[tokio::test]
async fn memory_prefix_revision_survives_local_writes_but_changes_on_same_tail_rebuild()
-> ConsoleLogResult<()> {
    assert_local_revision_contract(&InMemoryConsoleLogStore::new()).await
}

#[tokio::test]
async fn sqlite_prefix_revision_survives_local_writes_but_changes_on_same_tail_rebuild()
-> ConsoleLogResult<()> {
    let temp = tempfile::tempdir()?;
    let store = SqliteConsoleLogStore::open(temp.path().join("console.sqlite"))?;
    assert_local_revision_contract(&store).await
}

#[tokio::test]
async fn sqlite_prefix_revision_detects_external_commits_and_same_tail_rebuild()
-> ConsoleLogResult<()> {
    let temp = tempfile::tempdir()?;
    let path = temp.path().join("console.sqlite");
    let reader = SqliteConsoleLogStore::open(&path)?;
    let writer = SqliteConsoleLogStore::open(&path)?;
    let first = reader
        .append_if_absent(revision_frame("first", "original"))
        .await?
        .frame;
    reader
        .append_if_absent(revision_frame("middle", "unchanged"))
        .await?;
    let before_external_append = revision(&reader).await?;

    let tail = writer
        .append_if_absent(revision_frame("external-tail", "unchanged"))
        .await?
        .frame;
    let after_external_append = revision(&reader).await?;
    assert_ne!(after_external_append, before_external_append);
    assert_eq!(reader.latest_cursor().await?, Some(tail.cursor.clone()));
    assert_eq!(revision(&reader).await?, after_external_append);

    writer
        .update_frame_status(&first.id, ConsoleFrameStatus::DeliveryFailed)
        .await?
        .ok_or("external status update did not find the first frame")?;
    let after_external_status = revision(&reader).await?;
    assert_ne!(after_external_status, after_external_append);
    assert_eq!(reader.latest_cursor().await?, Some(tail.cursor.clone()));
    assert_eq!(
        reader
            .frame_by_dedupe_key("first")
            .await?
            .ok_or("reader did not observe the externally updated first frame")?
            .status,
        ConsoleFrameStatus::DeliveryFailed
    );

    // The reader deliberately does not observe the empty store between clear
    // and rebuild, and the rebuilt tail is byte-for-byte the same frame.
    writer.clear_frames().await?;
    writer
        .append_if_absent(revision_frame("first", "replacement"))
        .await?;
    writer
        .append_if_absent(revision_frame("middle", "unchanged"))
        .await?;
    writer
        .append_if_absent(revision_frame("external-tail", "unchanged"))
        .await?;
    let after_external_rebuild = revision(&reader).await?;
    assert_ne!(after_external_rebuild, after_external_status);
    assert_eq!(reader.latest_cursor().await?, Some(tail.cursor.clone()));
    assert_eq!(
        reader.frame_by_dedupe_key("external-tail").await?,
        Some(tail)
    );
    let replacement = reader
        .frame_by_dedupe_key("first")
        .await?
        .ok_or("reader did not observe the externally rebuilt first frame")?;
    assert_eq!(replacement.id, first.id);
    assert_eq!(replacement.cursor, first.cursor);
    assert_ne!(
        replacement.source.member_provenance,
        first.source.member_provenance
    );
    assert_eq!(revision(&reader).await?, after_external_rebuild);
    Ok(())
}
