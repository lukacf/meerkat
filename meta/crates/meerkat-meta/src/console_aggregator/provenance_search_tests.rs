use super::*;
use meerkat_core::types::SessionId;
use meerkat_mob::SpawnMemberSpec;
use std::sync::atomic::AtomicUsize;

const RUNTIME: &str = "provenance-search-runtime";
const IDENTITY: &str = "archived-worker";

struct SearchStore {
    inner: InMemoryConsoleLogStore,
    queries: std::sync::Mutex<Vec<Option<u64>>>,
    query_count: AtomicUsize,
    fail_query: AtomicUsize,
    supports_revision: bool,
    revision_count: AtomicUsize,
    replace_on_revision: AtomicUsize,
    replacement_rows: std::sync::Mutex<Vec<NewConsoleFrame>>,
}

struct SqliteAppendingSearchStore {
    inner: SqliteConsoleLogStore,
    writer: SqliteConsoleLogStore,
    session: String,
    queries: std::sync::Mutex<Vec<Option<u64>>>,
    query_count: AtomicUsize,
}

impl SqliteAppendingSearchStore {
    fn open(path: &std::path::Path, session: &str) -> ConsoleLogResult<Self> {
        Ok(Self {
            inner: SqliteConsoleLogStore::open(path)?,
            writer: SqliteConsoleLogStore::open(path)?,
            session: session.into(),
            queries: std::sync::Mutex::new(Vec::new()),
            query_count: AtomicUsize::new(0),
        })
    }

    fn query_afters(&self) -> ConsoleLogResult<Vec<Option<u64>>> {
        Ok(self
            .queries
            .lock()
            .map_err(|_| std::io::Error::other("SQLite provenance query log lock poisoned"))?
            .clone())
    }
}

#[async_trait::async_trait]
impl ConsoleLogStore for SqliteAppendingSearchStore {
    async fn append_if_absent(&self, frame: NewConsoleFrame) -> ConsoleLogResult<AppendOutcome> {
        self.inner.append_if_absent(frame).await
    }

    async fn update_frame_status(
        &self,
        id: &str,
        status: ConsoleFrameStatus,
    ) -> ConsoleLogResult<Option<ConsoleFrame>> {
        self.inner.update_frame_status(id, status).await
    }

    async fn query_frames(
        &self,
        query: ConsoleTimelineQuery,
    ) -> ConsoleLogResult<ConsoleTimelinePage> {
        let call = self.query_count.fetch_add(1, Ordering::SeqCst) + 1;
        self.queries
            .lock()
            .map_err(|_| std::io::Error::other("SQLite provenance query log lock poisoned"))?
            .push(query.after.as_ref().and_then(ConsoleCursor::seq));
        let before = self.inner.history_prefix_revision().await?;
        let page = self.inner.query_frames(query).await?;
        // Commit through a genuinely separate SQLite connection after the
        // page read, before recovery can inspect its closing revision. Every
        // scan gets a write, so retrying twice cannot hide the regression.
        self.writer
            .append_if_absent(history_row(
                &format!("external-append-during-query-{call}"),
                &self.session,
            ))
            .await?;
        assert_ne!(
            self.inner.history_prefix_revision().await?,
            before,
            "the external plain append must change the reader's SQLite data version"
        );
        Ok(page)
    }

    async fn query_windowed_frames(
        &self,
        query: ConsoleTimelineWindowQuery,
    ) -> ConsoleLogResult<ConsoleTimelineWindowPage> {
        // Public visibility queries use the same real store without injecting
        // extra writes unrelated to the provenance-recovery interleaving.
        self.inner.query_windowed_frames(query).await
    }

    async fn frame_by_dedupe_key(&self, key: &str) -> ConsoleLogResult<Option<ConsoleFrame>> {
        self.inner.frame_by_dedupe_key(key).await
    }

    async fn latest_cursor(&self) -> ConsoleLogResult<Option<ConsoleCursor>> {
        self.inner.latest_cursor().await
    }

    async fn history_prefix_revision(&self) -> ConsoleLogResult<Option<String>> {
        self.inner.history_prefix_revision().await
    }

    async fn clear_frames(&self) -> ConsoleLogResult<()> {
        self.inner.clear_frames().await
    }

    async fn record_source_watermark(
        &self,
        runtime: &str,
        kind: ConsoleFrameSourceKind,
        cursor: &str,
    ) -> ConsoleLogResult<()> {
        self.inner
            .record_source_watermark(runtime, kind, cursor)
            .await
    }

    async fn source_watermark(
        &self,
        runtime: &str,
        kind: ConsoleFrameSourceKind,
    ) -> ConsoleLogResult<Option<String>> {
        self.inner.source_watermark(runtime, kind).await
    }
}

impl SearchStore {
    fn new(supports_revision: bool) -> Self {
        Self {
            inner: InMemoryConsoleLogStore::default(),
            queries: std::sync::Mutex::new(Vec::new()),
            query_count: AtomicUsize::new(0),
            fail_query: AtomicUsize::new(0),
            supports_revision,
            revision_count: AtomicUsize::new(0),
            replace_on_revision: AtomicUsize::new(0),
            replacement_rows: std::sync::Mutex::new(Vec::new()),
        }
    }

    fn count(&self) -> usize {
        self.query_count.load(Ordering::SeqCst)
    }

    fn query_afters(&self) -> ConsoleLogResult<Vec<Option<u64>>> {
        Ok(self
            .queries
            .lock()
            .map_err(|_| std::io::Error::other("provenance query log lock poisoned"))?
            .clone())
    }
}

#[async_trait::async_trait]
impl ConsoleLogStore for SearchStore {
    async fn append_if_absent(&self, frame: NewConsoleFrame) -> ConsoleLogResult<AppendOutcome> {
        self.inner.append_if_absent(frame).await
    }

    async fn update_frame_status(
        &self,
        id: &str,
        status: ConsoleFrameStatus,
    ) -> ConsoleLogResult<Option<ConsoleFrame>> {
        self.inner.update_frame_status(id, status).await
    }

    async fn query_frames(
        &self,
        query: ConsoleTimelineQuery,
    ) -> ConsoleLogResult<ConsoleTimelinePage> {
        let call = self.query_count.fetch_add(1, Ordering::SeqCst) + 1;
        self.queries
            .lock()
            .map_err(|_| std::io::Error::other("provenance query log lock poisoned"))?
            .push(query.after.as_ref().and_then(ConsoleCursor::seq));
        if call == self.fail_query.load(Ordering::SeqCst) {
            return Err(std::io::Error::other("injected provenance page failure").into());
        }
        self.inner.query_frames(query).await
    }

    async fn frame_by_dedupe_key(&self, key: &str) -> ConsoleLogResult<Option<ConsoleFrame>> {
        self.inner.frame_by_dedupe_key(key).await
    }

    async fn latest_cursor(&self) -> ConsoleLogResult<Option<ConsoleCursor>> {
        self.inner.latest_cursor().await
    }

    async fn history_prefix_revision(&self) -> ConsoleLogResult<Option<String>> {
        let call = self.revision_count.fetch_add(1, Ordering::SeqCst) + 1;
        if call == self.replace_on_revision.load(Ordering::SeqCst) {
            let replacement = std::mem::take(&mut *self.replacement_rows.lock().map_err(|_| {
                std::io::Error::other("provenance replacement fixture lock poisoned")
            })?);
            self.inner.clear_frames().await?;
            for frame in replacement {
                self.inner.append_if_absent(frame).await?;
            }
        }
        if self.supports_revision {
            self.inner.history_prefix_revision().await
        } else {
            Ok(None)
        }
    }

    async fn clear_frames(&self) -> ConsoleLogResult<()> {
        self.inner.clear_frames().await
    }

    async fn record_source_watermark(
        &self,
        runtime: &str,
        kind: ConsoleFrameSourceKind,
        cursor: &str,
    ) -> ConsoleLogResult<()> {
        self.inner
            .record_source_watermark(runtime, kind, cursor)
            .await
    }

    async fn source_watermark(
        &self,
        runtime: &str,
        kind: ConsoleFrameSourceKind,
    ) -> ConsoleLogResult<Option<String>> {
        self.inner.source_watermark(runtime, kind).await
    }
}

fn aggregator_with_entry(
    store: Arc<dyn ConsoleLogStore>,
    runtime: &UnifiedRuntime,
) -> ConsoleLogResult<(MetaConsoleAggregator, RuntimeEntry)> {
    let aggregator = MetaConsoleAggregator::new_with_options(
        store,
        ConsoleAggregatorOptions {
            session_history_backfill_enabled: false,
            ..Default::default()
        },
    );
    let mut entry = super::tests::runtime_entry_for_test(RUNTIME, runtime);
    entry.identity_namespace.clear();
    aggregator
        .inner
        .runtimes
        .write()
        .map_err(|_| std::io::Error::other("provenance runtime registry lock poisoned"))?
        .insert(RUNTIME.into(), entry.clone());
    Ok((aggregator, entry))
}

fn witness(session: &str) -> ConsoleFrameMemberProvenance {
    let mut identity = super::tests::identity_record_for_test(IDENTITY);
    identity.runtime_key = RUNTIME.into();
    identity.session_id = Some(session.into());
    ConsoleFrameMemberProvenance {
        member: ConsoleMember {
            agent_identity: IDENTITY.into(),
            role: "worker".into(),
            state: "retired".into(),
            model_capabilities: Default::default(),
            runtime_mode: Some("turn_driven".into()),
            session_id: Some(session.into()),
            wired_to: Vec::new(),
            labels: BTreeMap::new(),
            progress: None,
        },
        identity,
        primary_mob_id: "historical-primary".into(),
        source_mob_id: "historical-primary".into(),
        lineage: Vec::new(),
    }
}

fn delegate_witness(session: &str) -> ConsoleFrameMemberProvenance {
    let mut provenance = witness(session);
    provenance.source_mob_id = "historical-delegate".into();
    provenance.member.role = "delegate".into();
    provenance.member.labels = BTreeMap::from([
        ("role".into(), "delegate".into()),
        ("source_mob_id".into(), provenance.source_mob_id.clone()),
    ]);
    provenance.identity.labels = provenance.member.labels.clone();
    provenance.identity.visibility = ConsoleVisibility::RetiredReadable;
    provenance
}

fn history_row(key: &str, session: &str) -> NewConsoleFrame {
    NewConsoleFrame {
        id: None,
        dedupe_key: key.into(),
        timestamp_ms: 1,
        runtime_key: RUNTIME.into(),
        identity: IDENTITY.into(),
        conversation_id: Some(IDENTITY.into()),
        session_id: Some(session.into()),
        kind: "text_complete".into(),
        status: ConsoleFrameStatus::Completed,
        payload: json!({"text": "Legacy history without a member witness."}),
        source: ConsoleFrameSource {
            member_provenance: None,
            kind: ConsoleFrameSourceKind::SessionHistory,
            source_cursor: Some(format!("{session}:0")),
        },
        source_event_id: None,
        interaction_id: None,
        turn_id: None,
        run_id: None,
        parent_frame_id: None,
        caused_by_frame_id: None,
    }
}

async fn lookup(
    aggregator: &MetaConsoleAggregator,
    entry: &RuntimeEntry,
    session: Option<&str>,
) -> Option<ConsoleFrameMemberProvenance> {
    member_provenance_for_identity(&aggregator.inner, entry, IDENTITY, session, false).await
}

#[tokio::test]
async fn provenance_sqlite_external_append_keeps_a_found_delegate_witness() -> ConsoleLogResult<()>
{
    let (_runtime_temp, runtime, _) = super::tests::build_stress_runtime(0, Duration::ZERO).await;
    let temp = tempfile::tempdir()?;
    let session = SessionId::new().to_string();
    let store = Arc::new(SqliteAppendingSearchStore::open(
        &temp.path().join("console.sqlite"),
        &session,
    )?);
    let (aggregator, entry) = aggregator_with_entry(store.clone(), &runtime)?;
    let expected = delegate_witness(&session);
    let mut retained = history_row("retained-delegate-witness", &session);
    retained.source.member_provenance = Some(expected.clone());
    store.append_if_absent(retained).await?;

    assert_eq!(
        lookup(&aggregator, &entry, Some(&session)).await,
        Some(expected.clone()),
        "a real external append cannot discard the already found private-member witness"
    );
    assert_eq!(
        store.query_afters()?,
        vec![None],
        "a positive witness needs no second full scan after an external append"
    );
    assert_eq!(
        lookup(&aggregator, &entry, Some(&session)).await,
        Some(expected)
    );
    assert_eq!(store.query_afters()?, vec![None]);
    runtime.mob_handle().stop().await?;
    Ok(())
}

#[tokio::test]
async fn provenance_sqlite_external_append_during_admission_keeps_delegate_history_hidden()
-> ConsoleLogResult<()> {
    let (_runtime_temp, runtime, _) = super::tests::build_stress_runtime(0, Duration::ZERO).await;
    let temp = tempfile::tempdir()?;
    let session = SessionId::new().to_string();
    let store = Arc::new(SqliteAppendingSearchStore::open(
        &temp.path().join("console.sqlite"),
        &session,
    )?);
    let (aggregator, _entry) = aggregator_with_entry(store.clone(), &runtime)?;
    let expected = delegate_witness(&session);
    let mut retained = history_row("retained-delegate-witness", &session);
    retained.source.member_provenance = Some(expected.clone());
    store.append_if_absent(retained).await?;

    // Admission must recover its own witness from a cold cache. A preceding
    // direct lookup would mask the privacy regression in this path.
    let outcome = append_and_emit_with_policy(
        &aggregator.inner,
        history_row("new-private-delegate-history", &session),
        Arc::new(AllowAllConsoleVisibilityPolicy),
    )
    .await?;
    let persisted = store
        .frame_by_dedupe_key("new-private-delegate-history")
        .await?
        .ok_or("admitted delegate history was not stored")?;
    let restricted =
        aggregator.policy_view(Arc::new(HideImplicitDelegateMembersConsoleVisibilityPolicy));
    assert!(
        !frame_is_visible(&restricted.inner, &persisted, true, &[]).await?,
        "a retired implicit delegate's newly admitted row must stay hidden after the external append"
    );
    assert_eq!(persisted, outcome.frame);
    assert_eq!(persisted.source.member_provenance, Some(expected));
    assert_eq!(
        store.query_afters()?,
        vec![None],
        "cold admission must retain its first positive search result"
    );
    let query = ConsoleTimelineQuery {
        identity: Some(IDENTITY.into()),
        after: Some(ConsoleCursor::from_seq(
            persisted
                .cursor
                .seq()
                .ok_or("stored cursor has no sequence")?
                - 1,
        )),
        limit: 100,
        ..Default::default()
    };
    assert!(
        restricted
            .query_timeline(query.clone())
            .await?
            .frames
            .is_empty()
    );
    assert!(
        aggregator
            .query_timeline(query)
            .await?
            .frames
            .iter()
            .any(|frame| frame.id == persisted.id),
        "the same admitted row remains readable under the open policy"
    );
    runtime.mob_handle().stop().await?;
    Ok(())
}

#[tokio::test]
async fn provenance_backfill_reuses_negative_prefix_and_recovers_a_later_witness()
-> ConsoleLogResult<()> {
    let (_temp, runtime, service) = super::tests::build_stress_runtime(0, Duration::ZERO).await;
    let store = Arc::new(SearchStore::new(true));
    let (aggregator, entry) = aggregator_with_entry(store.clone(), &runtime)?;
    let session = SessionId::new().to_string();
    let legacy = store
        .append_if_absent(history_row("legacy", &session))
        .await?
        .frame;
    let mut record = super::tests::identity_record_for_test(IDENTITY);
    record.runtime_key = RUNTIME.into();
    record.session_id = Some(session.clone());
    let target = SessionBackfillTarget {
        // Ordinary polling still resolves provenance before the fresh-watermark
        // gate. Recovery intentionally bypasses that gate to reconcile history.
        assistant_refresh: AssistantHistoryRefreshReason::PositiveOnly,
        provenance: None,
        entry: entry.clone(),
        record,
        session_id: session.clone(),
    };
    let reads = service.read_calls();
    for _ in 0..4 {
        // A fresh watermark isolates the actual backfill's provenance lookup
        // from unrelated session-history and runtime-notice projection work.
        store
            .record_source_watermark(
                &session_history_watermark_runtime_key(RUNTIME, &session),
                ConsoleFrameSourceKind::SessionHistory,
                &format_session_history_watermark(&session, 0, current_time_ms()),
            )
            .await?;
        backfill_one_session_history(aggregator.inner.clone(), target.clone(), false).await?;
    }
    assert_eq!(
        store.query_afters()?,
        vec![None],
        "one verified prefix scan across four real backfills"
    );
    assert_eq!(service.read_calls(), reads);
    let expected = witness(&session);
    let mut late = history_row("later-witness", &session);
    late.source.member_provenance = Some(expected.clone());
    store.append_if_absent(late).await?;
    assert_eq!(
        lookup(&aggregator, &entry, Some(&session)).await,
        Some(expected)
    );
    assert_eq!(store.query_afters()?, vec![None, legacy.cursor.seq()]);
    let queries = store.count();
    assert!(lookup(&aggregator, &entry, Some(&session)).await.is_some());
    assert_eq!(
        store.count(),
        queries,
        "a recovered positive witness is retained"
    );
    runtime.mob_handle().stop().await?;
    Ok(())
}

#[tokio::test]
async fn provenance_empty_prefix_scope_isolation_and_unsupported_revision_stay_correct()
-> ConsoleLogResult<()> {
    let (_temp, runtime, _) = super::tests::build_stress_runtime(0, Duration::ZERO).await;
    let store = Arc::new(SearchStore::new(true));
    let (aggregator, entry) = aggregator_with_entry(store.clone(), &runtime)?;
    let session = SessionId::new().to_string();
    assert!(lookup(&aggregator, &entry, Some(&session)).await.is_none());
    let first = store.count();
    assert!(lookup(&aggregator, &entry, Some(&session)).await.is_none());
    assert_eq!(
        store.count(),
        first,
        "empty unchanged store is a reusable prefix"
    );
    for mismatch in [
        "runtime",
        "identity",
        "session",
        "witness-runtime",
        "witness-identity",
        "witness-session",
    ] {
        let mut frame = history_row(mismatch, &session);
        let mut provenance = witness(&session);
        match mismatch {
            "runtime" => {
                frame.runtime_key = "foreign-runtime".into();
                provenance.identity.runtime_key = frame.runtime_key.clone();
            }
            "identity" => {
                frame.identity = "foreign-worker".into();
                provenance.identity.identity = frame.identity.clone();
                provenance.member.agent_identity = frame.identity.clone();
            }
            "session" => {
                frame.session_id = Some("foreign-session".into());
                provenance.identity.session_id = frame.session_id.clone();
                provenance.member.session_id = frame.session_id.clone();
            }
            "witness-runtime" => provenance.identity.runtime_key = "foreign-runtime".into(),
            "witness-identity" => provenance.identity.identity = "foreign-worker".into(),
            _ => provenance.identity.session_id = Some("foreign-session".into()),
        }
        frame.source.member_provenance = Some(provenance);
        store.append_if_absent(frame).await?;
        assert!(lookup(&aggregator, &entry, Some(&session)).await.is_none());
    }
    let expected = witness(&session);
    let mut matching = history_row("matching-after-empty", &session);
    matching.source.member_provenance = Some(expected.clone());
    store.append_if_absent(matching).await?;
    assert_eq!(
        lookup(&aggregator, &entry, Some(&session)).await,
        Some(expected.clone())
    );

    let unsupported = Arc::new(SearchStore::new(false));
    let (uncached, uncached_entry) = aggregator_with_entry(unsupported.clone(), &runtime)?;
    unsupported
        .append_if_absent(history_row("uncached-legacy", &session))
        .await?;
    assert!(
        lookup(&uncached, &uncached_entry, Some(&session))
            .await
            .is_none()
    );
    let scanned = unsupported.count();
    assert!(
        lookup(&uncached, &uncached_entry, Some(&session))
            .await
            .is_none()
    );
    assert!(
        unsupported.count() > scanned,
        "unknown revision cannot certify an unchanged prefix"
    );
    let mut matching = history_row("uncached-witness", &session);
    matching.source.member_provenance = Some(expected.clone());
    unsupported.append_if_absent(matching).await?;
    assert_eq!(
        lookup(&uncached, &uncached_entry, Some(&session)).await,
        Some(expected)
    );

    let default_store = Arc::new(SearchStore::new(true));
    let (default_aggregator, default_entry) =
        aggregator_with_entry(default_store.clone(), &runtime)?;
    assert!(
        lookup(&default_aggregator, &default_entry, Some(&session))
            .await
            .is_none()
    );
    let expected = witness(&session);
    let mut first_append = history_row("default-first-append", &session);
    first_append.source.member_provenance = Some(expected.clone());
    default_store.append_if_absent(first_append).await?;
    assert_eq!(
        lookup(&default_aggregator, &default_entry, Some(&session)).await,
        Some(expected),
        "Default must not assign the empty-prefix cursor to its first appended witness"
    );
    runtime.mob_handle().stop().await?;
    Ok(())
}

#[tokio::test]
async fn provenance_failed_page_and_same_cursor_same_tail_reset_cannot_cache_absence()
-> ConsoleLogResult<()> {
    let (_temp, runtime, _) = super::tests::build_stress_runtime(0, Duration::ZERO).await;
    let store = Arc::new(SearchStore::new(true));
    let (aggregator, entry) = aggregator_with_entry(store.clone(), &runtime)?;
    let session = SessionId::new().to_string();
    for index in 0..1_001 {
        store
            .append_if_absent(history_row(&format!("legacy-{index}"), &session))
            .await?;
    }
    store.fail_query.store(2, Ordering::SeqCst);
    assert!(lookup(&aggregator, &entry, Some(&session)).await.is_none());
    assert_eq!(
        store.count(),
        2,
        "the later page failed after a successful prefix read"
    );
    assert!(lookup(&aggregator, &entry, Some(&session)).await.is_none());
    assert_eq!(
        store.query_afters()?.get(2),
        Some(&None),
        "retry starts before the unverified prefix"
    );
    let verified = store.count();
    assert!(lookup(&aggregator, &entry, Some(&session)).await.is_none());
    assert_eq!(store.count(), verified);

    let reset_store = Arc::new(SearchStore::new(true));
    let (reset_aggregator, reset_entry) = aggregator_with_entry(reset_store.clone(), &runtime)?;
    reset_store
        .append_if_absent(history_row("old-first", &session))
        .await?;
    let tail = history_row("same-tail", &session);
    let original_tail = reset_store.append_if_absent(tail.clone()).await?.frame;
    assert!(
        lookup(&reset_aggregator, &reset_entry, Some(&session))
            .await
            .is_none()
    );
    reset_store.clear_frames().await?;
    let expected = witness(&session);
    let mut replacement = history_row("new-first", &session);
    replacement.source.member_provenance = Some(expected.clone());
    reset_store.append_if_absent(replacement).await?;
    let replacement_tail = reset_store.append_if_absent(tail).await?.frame;
    assert_eq!(
        replacement_tail, original_tail,
        "even the exact last frame cannot witness prefix continuity"
    );
    assert_eq!(
        lookup(&reset_aggregator, &reset_entry, Some(&session)).await,
        Some(expected)
    );
    assert_eq!(
        reset_store.query_afters()?.last(),
        Some(&None),
        "reset must search the replaced prefix"
    );

    let raced_store = Arc::new(SearchStore::new(true));
    let (raced_aggregator, raced_entry) = aggregator_with_entry(raced_store.clone(), &runtime)?;
    raced_store
        .append_if_absent(history_row("raced-old", &session))
        .await?;
    let expected = witness(&session);
    let mut replacement = history_row("raced-witness", &session);
    replacement.source.member_provenance = Some(expected.clone());
    raced_store
        .replacement_rows
        .lock()
        .map_err(|_| std::io::Error::other("provenance replacement fixture lock poisoned"))?
        .push(replacement);
    raced_store.replace_on_revision.store(2, Ordering::SeqCst);
    assert_eq!(
        lookup(&raced_aggregator, &raced_entry, Some(&session)).await,
        Some(expected),
        "a reset between the scan and its revision check must retry the new prefix"
    );
    assert_eq!(raced_store.query_afters()?, vec![None, None]);
    runtime.mob_handle().stop().await?;
    Ok(())
}

/// The session `identity` is live under in `entry`'s runtime, as the console
/// reads its members.
async fn live_session(entry: &RuntimeEntry, identity: &str) -> Option<String> {
    for resolved in member_sources_for_entry_including_hidden(entry).await {
        if let Some(record) = identity_record_for_resolved_member(&resolved).await
            && record.identity == identity
        {
            return record.session_id;
        }
    }
    None
}

/// A negative history search never masks a member that turns live, nor a
/// new registration. A lookup names the frame's session: a frame that names
/// no session records no member, so a session-less lookup is uncaptured
/// before and after the spawn. The live member is found under its own
/// session although a negative search is recorded for that session.
#[tokio::test]
async fn provenance_negative_search_never_masks_live_members_or_a_new_registration()
-> ConsoleLogResult<()> {
    let (_temp, runtime, _) = super::tests::build_stress_runtime(0, Duration::ZERO).await;
    let store = Arc::new(SearchStore::new(true));
    let (aggregator, entry) = aggregator_with_entry(store.clone(), &runtime)?;
    assert!(
        member_provenance_for_identity(&aggregator.inner, &entry, "live-worker", None, false)
            .await
            .is_none(),
        "a session-less lookup is uncaptured"
    );
    runtime
        .spawn(SpawnMemberSpec::from_wire(
            "worker".into(),
            "live-worker".into(),
            Some("Ready.".into()),
            None,
            None,
        ))
        .await?;
    let session = live_session(&entry, "live-worker")
        .await
        .ok_or("the spawned member is live with a session")?;
    // The negative history search that a lookup under this session leaves
    // when it runs before the member is live.
    let key = (
        entry.runtime_key.clone(),
        "live-worker".to_string(),
        Some(session.clone()),
    );
    assert!(
        recover_member_provenance(
            &aggregator.inner,
            &entry,
            "live-worker",
            Some(session.as_str()),
            key.clone(),
        )
        .await
        .is_none()
    );
    assert!(
        aggregator
            .inner
            .member_provenance_searches
            .lock()
            .map_err(|_| std::io::Error::other("provenance search cache lock poisoned"))?
            .entries
            .contains_key(&key),
        "the negative search is recorded under the member's session"
    );
    let queries = store.count();
    let live = member_provenance_for_identity(
        &aggregator.inner,
        &entry,
        "live-worker",
        Some(session.as_str()),
        false,
    )
    .await
    .ok_or("new live member was masked by a negative history search")?;
    assert_eq!(live.identity.identity, "live-worker");
    assert_eq!(live.identity.runtime_key, RUNTIME);
    assert_eq!(live.identity.session_id.as_deref(), Some(session.as_str()));
    assert_eq!(
        store.count(),
        queries,
        "live resolution precedes the negative history shortcut"
    );
    assert!(
        member_provenance_for_identity(&aggregator.inner, &entry, "live-worker", None, false)
            .await
            .is_none(),
        "a session-less lookup stays uncaptured once the member is live"
    );
    runtime.mob_handle().stop().await?;

    // Register an empty runtime so unrelated live-event replay cannot affect
    // the query-count assertion for the replaced registration.
    let (_registration_temp, runtime, _) =
        super::tests::build_stress_runtime(0, Duration::ZERO).await;
    let store = Arc::new(SearchStore::new(true));
    let (aggregator, entry) = aggregator_with_entry(store.clone(), &runtime)?;
    let session = SessionId::new().to_string();
    store
        .append_if_absent(history_row("registration-prefix", &session))
        .await?;
    assert!(lookup(&aggregator, &entry, Some(&session)).await.is_none());
    let before = store.count();
    assert!(lookup(&aggregator, &entry, Some(&session)).await.is_none());
    assert_eq!(store.count(), before);
    aggregator.register_runtime_handles_with_policy(
        RUNTIME,
        "",
        runtime.mob_runtime().clone(),
        None,
        runtime.console_events(),
        Arc::new(AllowAllConsoleVisibilityPolicy),
    );
    let replacement = aggregator
        .inner
        .runtimes
        .read()
        .map_err(|_| std::io::Error::other("replacement runtime registry lock poisoned"))?
        .get(RUNTIME)
        .cloned()
        .ok_or("replacement runtime entry missing")?;
    assert_ne!(replacement.registration_id, entry.registration_id);
    assert!(
        lookup(&aggregator, &replacement, Some(&session))
            .await
            .is_none()
    );
    assert!(
        store.count() > before,
        "new runtime registration cannot reuse predecessor absence"
    );
    assert_eq!(store.query_afters()?.get(before), Some(&None));
    aggregator.unregister_runtime(RUNTIME);
    runtime.mob_handle().stop().await?;
    Ok(())
}
