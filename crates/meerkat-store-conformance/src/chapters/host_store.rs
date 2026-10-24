//! Host store chapter: the obligations of `meerkat_core::host_store`,
//! checked through the six operations only.
//!
//! Every [`HostStoreFactory::open`] handle stands for one process. A row
//! kills a process by dropping its handle, with any commit it has in flight,
//! and opens another over the same realm. Where the contract lets a store
//! meet an obligation either by refusing at commit or by resolving at load,
//! the rows accept both.

use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
#[cfg(not(target_arch = "wasm32"))]
use std::time::Duration;

use meerkat_core::host_store::{
    AcquireMode, CommitBatch, CommitId, FenceToken, HostStore, HostStoreError, MAX_BATCH_BYTES,
    MAX_ROW_BYTES, Row, RowIndex, ScopeHead, ScopeKind, ScopeName, ScopeRef, ScopeUpdate,
    ScopeVersion, VersionedHead,
};

use crate::factory::HostStoreFactory;
use crate::failure::{ConformanceFailure, Steps};

const CHAPTER: &str = "host_store";

/// A limit large enough to read every row any scope of this chapter holds.
const ALL_ROWS: usize = 1_000;

/// The most pages a listing may take before the row calls it endless.
const MAX_PAGES: usize = 64;

const V2: ScopeVersion = ScopeVersion::new(2);
const V3: ScopeVersion = ScopeVersion::new(3);

/// Host store chapter. Runs every row against one realm, in order; each row
/// that restarts the writer hands the next row the new one.
pub async fn host_store(factory: &dyn HostStoreFactory) -> Result<(), ConformanceFailure> {
    let mut chapter = Chapter {
        steps: Steps::chapter(CHAPTER),
        factory,
        minted: 0,
    };
    let writer = chapter.acquire_on_empty_realm().await?;
    reopen(&mut chapter, &writer).await?;
    let writer = kill(&mut chapter, writer).await?;
    let writer = stale_fence(&mut chapter, writer).await?;
    let writer = takeover_unknown_commit(&mut chapter, writer).await?;
    same_version_twice(&mut chapter, &writer).await?;
    retry(&mut chapter, &writer).await?;
    rows_removal_listing(&mut chapter, &writer).await?;
    #[cfg(not(target_arch = "wasm32"))]
    independence(&mut chapter, &writer).await?;
    #[cfg(not(target_arch = "wasm32"))]
    stalled_commit_does_not_block(&mut chapter, &writer).await?;
    Ok(())
}

/// One writer process: a handle and the fence token it acquired.
struct Writer {
    store: Arc<dyn HostStore>,
    token: FenceToken,
}

struct Chapter<'a> {
    steps: Steps,
    factory: &'a dyn HostStoreFactory,
    minted: u64,
}

impl Chapter<'_> {
    /// A commit id unique in the realm: the step's name and a sequence number.
    fn mint(&mut self, step: &'static str) -> Result<CommitId, ConformanceFailure> {
        self.minted += 1;
        self.steps
            .wrap(step, CommitId::try_new(format!("{step}-{}", self.minted)))
    }

    /// Commit `updates` as one batch under `writer`'s token; it must land.
    async fn commit(
        &self,
        step: &'static str,
        writer: &Writer,
        commit_id: CommitId,
        updates: Vec<ScopeUpdate>,
    ) -> Result<(), ConformanceFailure> {
        let batch = batch(&self.steps, step, commit_id, writer, updates)?;
        self.steps.wrap(step, writer.store.commit(&batch).await)
    }

    async fn acquire_on_empty_realm(&self) -> Result<Writer, ConformanceFailure> {
        const STEP: &str = "acquire_on_empty_realm";
        let store = self.factory.open().await?;
        let grant = self
            .steps
            .wrap(STEP, store.acquire_fence(AcquireMode::Exclusive).await)?;
        self.steps.ensure(
            STEP,
            grant.superseded_lease.is_zero(),
            format!(
                "an acquire on an empty realm supersedes no lease, got {:?}",
                grant.superseded_lease
            ),
        )?;
        Ok(Writer {
            store,
            token: grant.token,
        })
    }

    /// A new process takes the fence over from the holder of `previous`: its
    /// token is higher, and renewing `previous` answers `FencedOut` with it.
    async fn takeover(
        &self,
        step: &'static str,
        previous: FenceToken,
    ) -> Result<Writer, ConformanceFailure> {
        let store = self.factory.open().await?;
        let grant = self
            .steps
            .wrap(step, store.acquire_fence(AcquireMode::Takeover).await)?;
        self.steps.ensure(
            step,
            grant.token > previous,
            format!(
                "a takeover must answer a token above {previous}, got {}",
                grant.token
            ),
        )?;
        let renewed = store.renew_fence(previous).await;
        self.steps.ensure(
            step,
            renewed
                == Err(HostStoreError::FencedOut {
                    current: grant.token,
                }),
            format!(
                "renewing token {previous} after the takeover must answer FencedOut with token {}, got {renewed:?}",
                grant.token
            ),
        )?;
        Ok(Writer {
            store,
            token: grant.token,
        })
    }
}

fn scope_ref(
    steps: &Steps,
    step: &'static str,
    kind: ScopeKind,
    name: &str,
) -> Result<ScopeRef, ConformanceFailure> {
    Ok(ScopeRef::new(
        kind,
        steps.wrap(step, ScopeName::try_new(name))?,
    ))
}

fn rows(steps: &Steps, step: &'static str, items: &[&str]) -> Result<Vec<Row>, ConformanceFailure> {
    items
        .iter()
        .map(|item| steps.wrap(step, Row::try_new(item.as_bytes().to_vec())))
        .collect()
}

fn rows_are(read: &[Row], expected: &[&str]) -> bool {
    read.len() == expected.len()
        && read
            .iter()
            .zip(expected)
            .all(|(row, item)| row.as_bytes() == item.as_bytes())
}

fn batch(
    steps: &Steps,
    step: &'static str,
    commit_id: CommitId,
    writer: &Writer,
    updates: Vec<ScopeUpdate>,
) -> Result<CommitBatch, ConformanceFailure> {
    steps.wrap(step, CommitBatch::try_new(commit_id, writer.token, updates))
}

/// An update that moves `scope` from `previous` to the next version and
/// changes nothing else.
fn change(scope: &ScopeRef, previous: ScopeVersion) -> ScopeUpdate {
    ScopeUpdate {
        scope: scope.clone(),
        previous: Some(previous),
        next: ScopeVersion::new(previous.get().saturating_add(1)),
        head: None,
        append: Vec::new(),
        truncate_after: None,
        drop_before: None,
        delete: false,
    }
}

fn create(scope: &ScopeRef, head: &str, append: Vec<Row>) -> ScopeUpdate {
    ScopeUpdate {
        scope: scope.clone(),
        previous: None,
        next: ScopeVersion::FIRST,
        head: Some(head.as_bytes().to_vec()),
        append,
        truncate_after: None,
        drop_before: None,
        delete: false,
    }
}

fn update(
    scope: &ScopeRef,
    previous: ScopeVersion,
    head: Option<&str>,
    append: Vec<Row>,
) -> ScopeUpdate {
    ScopeUpdate {
        head: head.map(|head| head.as_bytes().to_vec()),
        append,
        ..change(scope, previous)
    }
}

/// What one scope shows: its visible head and its rows from index zero.
#[derive(Debug, PartialEq, Eq)]
struct Shown {
    head: Option<VersionedHead>,
    rows: Vec<Row>,
}

fn shown(version: ScopeVersion, commit_id: &CommitId, head: &str, rows: Vec<Row>) -> Shown {
    Shown {
        head: Some(VersionedHead {
            version,
            commit_id: commit_id.clone(),
            head: head.as_bytes().to_vec(),
        }),
        rows,
    }
}

async fn load_shown(
    steps: &Steps,
    step: &'static str,
    store: &dyn HostStore,
    scope: &ScopeRef,
) -> Result<Shown, ConformanceFailure> {
    let head = steps.wrap(step, store.load_head(scope).await)?;
    let rows = steps.wrap(
        step,
        store.read_rows(scope, RowIndex::new(0), ALL_ROWS).await,
    )?;
    Ok(Shown { head, rows })
}

async fn ensure_shows(
    steps: &Steps,
    step: &'static str,
    store: &dyn HostStore,
    scope: &ScopeRef,
    expected: &Shown,
    what: &str,
) -> Result<(), ConformanceFailure> {
    let now = load_shown(steps, step, store, scope).await?;
    steps.ensure(
        step,
        now == *expected,
        format!("{what}: {scope} must show {expected:?}, shows {now:?}"),
    )
}

/// A future that wakes itself and answers `Pending` once: a portable yield
/// to the executor between two polls of a commit.
#[derive(Default)]
struct YieldOnce {
    yielded: bool,
}

impl Future for YieldOnce {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        if self.yielded {
            Poll::Ready(())
        } else {
            self.yielded = true;
            cx.waker().wake_by_ref();
            Poll::Pending
        }
    }
}

/// Poll a commit at most `polls` times with the task's own context, so a real
/// store makes progress, yielding once between polls. Answers the outcome if
/// the commit finished, and leaves it unfinished otherwise.
async fn poll_up_to<F>(
    pending: &mut Pin<Box<F>>,
    polls: usize,
) -> Option<Result<(), HostStoreError>>
where
    F: Future<Output = Result<(), HostStoreError>> + ?Sized,
{
    for poll in 0..polls {
        if poll > 0 {
            YieldOnce::default().await;
        }
        let outcome = std::future::poll_fn(|cx| match pending.as_mut().poll(cx) {
            Poll::Ready(outcome) => Poll::Ready(Some(outcome)),
            Poll::Pending => Poll::Ready(None),
        })
        .await;
        if outcome.is_some() {
            return outcome;
        }
    }
    None
}

/// A new handle loads what an earlier one committed: heads, versions, commit
/// ids and rows; an absent scope loads nothing.
async fn reopen(chapter: &mut Chapter<'_>, writer: &Writer) -> Result<(), ConformanceFailure> {
    const STEP: &str = "reopen";
    let steps = chapter.steps;
    let a = scope_ref(&steps, STEP, ScopeKind::Session, "reopen-a")?;
    let b = scope_ref(&steps, STEP, ScopeKind::Mob, "reopen-b")?;
    let absent = scope_ref(&steps, STEP, ScopeKind::Session, "reopen-absent")?;
    let first = chapter.mint(STEP)?;
    let created = vec![
        create(&a, "a1", rows(&steps, STEP, &["a0"])?),
        create(&b, "b1", rows(&steps, STEP, &["b0", "b1"])?),
    ];
    chapter.commit(STEP, writer, first, created).await?;
    let second = chapter.mint(STEP)?;
    let updated = vec![
        update(
            &a,
            ScopeVersion::FIRST,
            Some("a2"),
            rows(&steps, STEP, &["a1"])?,
        ),
        update(
            &b,
            ScopeVersion::FIRST,
            Some("b2"),
            rows(&steps, STEP, &["b2"])?,
        ),
    ];
    chapter
        .commit(STEP, writer, second.clone(), updated)
        .await?;
    let reopened = chapter.factory.open().await?;
    let what = "a new handle loads what an earlier one committed";
    let a_shows = shown(V2, &second, "a2", rows(&steps, STEP, &["a0", "a1"])?);
    ensure_shows(&steps, STEP, reopened.as_ref(), &a, &a_shows, what).await?;
    let b_shows = shown(V2, &second, "b2", rows(&steps, STEP, &["b0", "b1", "b2"])?);
    ensure_shows(&steps, STEP, reopened.as_ref(), &b, &b_shows, what).await?;
    let nothing = Shown {
        head: None,
        rows: Vec::new(),
    };
    ensure_shows(
        &steps,
        STEP,
        reopened.as_ref(),
        &absent,
        &nothing,
        "a scope never created loads nothing",
    )
    .await
}

/// Where a writer dies relative to its commit.
#[derive(Debug, Clone, Copy)]
enum Cut {
    /// The commit future is never polled.
    Before,
    /// The commit future is polled this many times, then dropped.
    During(usize),
    /// The commit is awaited to its answer.
    After,
}

const CUTS: [Cut; 9] = [
    Cut::Before,
    Cut::During(1),
    Cut::During(2),
    Cut::During(3),
    Cut::During(5),
    Cut::During(8),
    Cut::During(13),
    Cut::During(34),
    Cut::After,
];

/// A writer dies before, during or after a batch over two scopes: the next
/// writer sees the batch in both scopes or in neither, an acknowledged batch
/// in both, and settles an unknown batch the way Core does.
async fn kill(chapter: &mut Chapter<'_>, mut writer: Writer) -> Result<Writer, ConformanceFailure> {
    const STEP: &str = "kill";
    let steps = chapter.steps;
    for (case, cut) in CUTS.into_iter().enumerate() {
        let s = scope_ref(&steps, STEP, ScopeKind::Session, &format!("kill-{case}-s"))?;
        let t = scope_ref(&steps, STEP, ScopeKind::Mob, &format!("kill-{case}-t"))?;
        let base_id = chapter.mint(STEP)?;
        let created = vec![
            create(&s, "base", rows(&steps, STEP, &["r0"])?),
            create(&t, "base", rows(&steps, STEP, &["r0"])?),
        ];
        chapter
            .commit(STEP, &writer, base_id.clone(), created)
            .await?;
        let k_id = chapter.mint(STEP)?;
        let k = batch(
            &steps,
            STEP,
            k_id.clone(),
            &writer,
            vec![
                update(
                    &s,
                    ScopeVersion::FIRST,
                    Some("k"),
                    rows(&steps, STEP, &["k0", "k1"])?,
                ),
                update(
                    &t,
                    ScopeVersion::FIRST,
                    Some("k"),
                    rows(&steps, STEP, &["k0"])?,
                ),
            ],
        )?;
        let answered = match cut {
            Cut::Before => {
                drop(writer.store.commit(&k));
                None
            }
            Cut::During(polls) => {
                let mut pending = writer.store.commit(&k);
                let answered = poll_up_to(&mut pending, polls).await;
                drop(pending);
                answered
            }
            Cut::After => Some(writer.store.commit(&k).await),
        };
        let previous = writer.token;
        drop(writer);
        let next = chapter.takeover(STEP, previous).await?;

        let base_s = shown(
            ScopeVersion::FIRST,
            &base_id,
            "base",
            rows(&steps, STEP, &["r0"])?,
        );
        let base_t = shown(
            ScopeVersion::FIRST,
            &base_id,
            "base",
            rows(&steps, STEP, &["r0"])?,
        );
        let k_s = shown(V2, &k_id, "k", rows(&steps, STEP, &["r0", "k0", "k1"])?);
        let k_t = shown(V2, &k_id, "k", rows(&steps, STEP, &["r0", "k0"])?);
        let now_s = load_shown(&steps, STEP, next.store.as_ref(), &s).await?;
        let now_t = load_shown(&steps, STEP, next.store.as_ref(), &t).await?;
        let landed = now_s == k_s && now_t == k_t;
        let kept = now_s == base_s && now_t == base_t;
        steps.ensure(
            STEP,
            landed || kept,
            format!(
                "{cut:?}: a batch shows in all its scopes or in none, and its rows once; {s} shows {now_s:?}, {t} shows {now_t:?}"
            ),
        )?;
        match &answered {
            Some(Ok(())) => steps.ensure(
                STEP,
                landed,
                format!("{cut:?}: an acknowledged batch survives its writer's death"),
            )?,
            Some(Err(HostStoreError::Indeterminate(_))) | None => {}
            Some(Err(error)) => steps.ensure(
                STEP,
                kept,
                format!("{cut:?}: a commit refused with {error} applies nothing"),
            )?,
        }
        let fresh_id = chapter.mint(STEP)?;
        if landed {
            let continued = vec![
                update(&s, V2, Some("fresh"), rows(&steps, STEP, &["f0"])?),
                update(&t, V2, Some("fresh"), rows(&steps, STEP, &["f0"])?),
            ];
            chapter
                .commit(STEP, &next, fresh_id.clone(), continued)
                .await?;
            let what = "the new writer continues from the landed batch";
            let s_shows = shown(
                V3,
                &fresh_id,
                "fresh",
                rows(&steps, STEP, &["r0", "k0", "k1", "f0"])?,
            );
            ensure_shows(&steps, STEP, next.store.as_ref(), &s, &s_shows, what).await?;
            let t_shows = shown(
                V3,
                &fresh_id,
                "fresh",
                rows(&steps, STEP, &["r0", "k0", "f0"])?,
            );
            ensure_shows(&steps, STEP, next.store.as_ref(), &t, &t_shows, what).await?;
        } else {
            let retried = next.store.commit(&k).await;
            let what = format!(
                "{cut:?}: retrying an unlanded batch under the superseded token must not make it visible (answered {retried:?})"
            );
            ensure_shows(&steps, STEP, next.store.as_ref(), &s, &base_s, &what).await?;
            ensure_shows(&steps, STEP, next.store.as_ref(), &t, &base_t, &what).await?;
            let settled = vec![
                update(
                    &s,
                    ScopeVersion::FIRST,
                    Some("fresh"),
                    rows(&steps, STEP, &["f0"])?,
                ),
                update(
                    &t,
                    ScopeVersion::FIRST,
                    Some("fresh"),
                    rows(&steps, STEP, &["f0"])?,
                ),
            ];
            chapter
                .commit(STEP, &next, fresh_id.clone(), settled)
                .await?;
            let what = "a fresh batch at the same version under the new token lands";
            let s_shows = shown(V2, &fresh_id, "fresh", rows(&steps, STEP, &["r0", "f0"])?);
            ensure_shows(&steps, STEP, next.store.as_ref(), &s, &s_shows, what).await?;
            let t_shows = shown(V2, &fresh_id, "fresh", rows(&steps, STEP, &["r0", "f0"])?);
            ensure_shows(&steps, STEP, next.store.as_ref(), &t, &t_shows, what).await?;
        }
        writer = next;
    }
    Ok(writer)
}

/// A superseded writer can neither keep the fence nor make a batch visible;
/// an exclusive acquire waits out a live holder's lease and no longer.
///
/// The row's first renew is the current holder's, which may have sat idle
/// past its lease during the rows before it. It must succeed: a store
/// refuses only a superseded token or one it never issued. A holder that
/// stopped re-acquires instead of resuming on a renew, and that rule is
/// Core's, not the store's.
async fn stale_fence(chapter: &mut Chapter<'_>, a: Writer) -> Result<Writer, ConformanceFailure> {
    const STEP: &str = "stale_fence";
    let steps = chapter.steps;
    // The current holder renews, whether or not its lease lapsed meanwhile.
    steps.wrap(STEP, a.store.renew_fence(a.token).await)?;
    let f = scope_ref(&steps, STEP, ScopeKind::Session, "fence-f")?;
    let first_id = chapter.mint(STEP)?;
    chapter
        .commit(
            STEP,
            &a,
            first_id.clone(),
            vec![create(&f, "a1", Vec::new())],
        )
        .await?;
    let first = shown(ScopeVersion::FIRST, &first_id, "a1", Vec::new());

    let b_store = chapter.factory.open().await?;
    let held = b_store.acquire_fence(AcquireMode::Exclusive).await;
    steps.ensure(
        STEP,
        matches!(held, Err(HostStoreError::Held { holder, remaining }) if holder == a.token && !remaining.is_zero()),
        format!(
            "an exclusive acquire beside the live holder {} must answer Held by it with lease left, got {held:?}",
            a.token
        ),
    )?;
    let grant = steps.wrap(STEP, b_store.acquire_fence(AcquireMode::Takeover).await)?;
    steps.ensure(
        STEP,
        grant.token > a.token && !grant.superseded_lease.is_zero(),
        format!(
            "a takeover of the live holder {} must answer a higher token and the holder's remaining lease, got {grant:?}",
            a.token
        ),
    )?;
    let b = Writer {
        store: b_store,
        token: grant.token,
    };
    ensure_shows(
        &steps,
        STEP,
        b.store.as_ref(),
        &f,
        &first,
        "a load after an acquire shows every batch acknowledged before it",
    )
    .await?;
    let renewed = a.store.renew_fence(a.token).await;
    steps.ensure(
        STEP,
        renewed == Err(HostStoreError::FencedOut { current: b.token }),
        format!(
            "the superseded holder's renew must answer FencedOut with token {}, got {renewed:?}",
            b.token
        ),
    )?;

    let stale_id = chapter.mint(STEP)?;
    let stale = batch(
        &steps,
        STEP,
        stale_id,
        &a,
        vec![update(&f, ScopeVersion::FIRST, Some("a2"), Vec::new())],
    )?;
    let answered = a.store.commit(&stale).await;
    steps.ensure(
        STEP,
        matches!(answered, Ok(()) | Err(HostStoreError::FencedOut { .. })),
        format!("a commit under a superseded token must answer FencedOut or land invisibly, got {answered:?}"),
    )?;
    let witness = chapter.factory.open().await?;
    for store in [b.store.as_ref(), witness.as_ref()] {
        ensure_shows(
            &steps,
            STEP,
            store,
            &f,
            &first,
            "a batch under a superseded token never becomes visible",
        )
        .await?;
    }
    let b_id = chapter.mint(STEP)?;
    chapter
        .commit(
            STEP,
            &b,
            b_id.clone(),
            vec![update(&f, ScopeVersion::FIRST, Some("b2"), Vec::new())],
        )
        .await?;
    let second = shown(V2, &b_id, "b2", Vec::new());
    for store in [b.store.as_ref(), witness.as_ref()] {
        ensure_shows(
            &steps,
            STEP,
            store,
            &f,
            &second,
            "a lower token never beats a higher one at the same version",
        )
        .await?;
    }
    let again = a.store.commit(&stale).await;
    steps.ensure(
        STEP,
        matches!(
            again,
            Ok(()) | Err(HostStoreError::FencedOut { .. } | HostStoreError::Conflict { .. })
        ),
        format!(
            "a retry under a superseded token must be refused or land invisibly, got {again:?}"
        ),
    )?;
    for store in [b.store.as_ref(), witness.as_ref()] {
        ensure_shows(
            &steps,
            STEP,
            store,
            &f,
            &second,
            "a retried batch under a superseded token stays invisible",
        )
        .await?;
    }
    drop(a);

    // Renew right before the exclusive acquire, so that a compliant store
    // with a short lease, or slow calls since the takeover, still holds B.
    steps.wrap(STEP, b.store.renew_fence(b.token).await)?;
    let c_store = chapter.factory.open().await?;
    let held = c_store.acquire_fence(AcquireMode::Exclusive).await;
    steps.ensure(
        STEP,
        matches!(held, Err(HostStoreError::Held { holder, remaining }) if holder == b.token && !remaining.is_zero()),
        format!(
            "an exclusive acquire beside the live holder {} must answer Held by it, got {held:?}",
            b.token
        ),
    )?;
    chapter.factory.lapse_fence_leases().await?;
    let grant = steps.wrap(STEP, c_store.acquire_fence(AcquireMode::Exclusive).await)?;
    steps.ensure(
        STEP,
        grant.token > b.token && grant.superseded_lease.is_zero(),
        format!(
            "an exclusive acquire after every lease lapsed must answer a token above {} and supersede no lease, got {grant:?}",
            b.token
        ),
    )?;
    let renewed = b.store.renew_fence(b.token).await;
    steps.ensure(
        STEP,
        renewed
            == Err(HostStoreError::FencedOut {
                current: grant.token,
            }),
        format!(
            "the superseded holder's renew must answer FencedOut with token {}, got {renewed:?}",
            grant.token
        ),
    )?;
    drop(b);
    Ok(Writer {
        store: c_store,
        token: grant.token,
    })
}

/// A takeover lands while the old writer's commit is in flight: the commit
/// shows wholly or never, the same to every load, and the new writer
/// continues from what it loads.
async fn takeover_unknown_commit(
    chapter: &mut Chapter<'_>,
    mut writer: Writer,
) -> Result<Writer, ConformanceFailure> {
    const STEP: &str = "takeover_unknown_commit";
    let steps = chapter.steps;
    for polls in [0_usize, 1, 3] {
        let u = scope_ref(
            &steps,
            STEP,
            ScopeKind::Session,
            &format!("unknown-{polls}"),
        )?;
        let base_id = chapter.mint(STEP)?;
        chapter
            .commit(
                STEP,
                &writer,
                base_id.clone(),
                vec![create(&u, "base", Vec::new())],
            )
            .await?;
        let unknown_id = chapter.mint(STEP)?;
        let unknown = batch(
            &steps,
            STEP,
            unknown_id.clone(),
            &writer,
            vec![update(
                &u,
                ScopeVersion::FIRST,
                Some("unknown"),
                rows(&steps, STEP, &["u0"])?,
            )],
        )?;
        let base = shown(ScopeVersion::FIRST, &base_id, "base", Vec::new());
        let landed = shown(V2, &unknown_id, "unknown", rows(&steps, STEP, &["u0"])?);

        let mut pending = writer.store.commit(&unknown);
        let early = poll_up_to(&mut pending, polls).await;
        let acknowledged_early = matches!(early, Some(Ok(())));
        let next = chapter.takeover(STEP, writer.token).await?;
        let answered = if let Some(answered) = early {
            drop(pending);
            answered
        } else {
            pending.await
        };
        drop(writer);

        let witness = chapter.factory.open().await?;
        let views = [
            load_shown(&steps, STEP, next.store.as_ref(), &u).await?,
            load_shown(&steps, STEP, next.store.as_ref(), &u).await?,
            load_shown(&steps, STEP, witness.as_ref(), &u).await?,
        ];
        steps.ensure(
            STEP,
            views.iter().all(|view| *view == views[0]),
            format!("{polls} polls: every load of {u} after the takeover agrees: {views:?}"),
        )?;
        let shows_unknown = views[0] == landed;
        steps.ensure(
            STEP,
            shows_unknown || views[0] == base,
            format!(
                "{polls} polls: {u} shows its base or the whole batch, shows {:?}",
                views[0]
            ),
        )?;
        steps.ensure(
            STEP,
            shows_unknown || !acknowledged_early,
            format!("{polls} polls: a batch acknowledged before the takeover stays visible"),
        )?;
        steps.ensure(
            STEP,
            !(shows_unknown && matches!(answered, Err(HostStoreError::FencedOut { .. }))),
            format!("{polls} polls: a batch answered FencedOut never shows"),
        )?;

        let next_id = chapter.mint(STEP)?;
        let (from, continued) = if shows_unknown {
            (
                V2,
                shown(V3, &next_id, "taken", rows(&steps, STEP, &["u0", "x0"])?),
            )
        } else {
            (
                ScopeVersion::FIRST,
                shown(V2, &next_id, "taken", rows(&steps, STEP, &["x0"])?),
            )
        };
        chapter
            .commit(
                STEP,
                &next,
                next_id,
                vec![update(
                    &u,
                    from,
                    Some("taken"),
                    rows(&steps, STEP, &["x0"])?,
                )],
            )
            .await?;
        ensure_shows(
            &steps,
            STEP,
            next.store.as_ref(),
            &u,
            &continued,
            "the new writer commits the next version of what it loads",
        )
        .await?;
        writer = next;
    }
    Ok(writer)
}

type Loaded = Result<Option<VersionedHead>, HostStoreError>;

fn written_by(load: &Loaded) -> Option<&CommitId> {
    match load {
        Ok(Some(head)) => Some(&head.commit_id),
        _ => None,
    }
}

/// Two batches for one version of a scope, under one token: every load
/// resolves them the same way, and the batch that loses is invisible in all
/// its scopes.
async fn same_version_twice(
    chapter: &mut Chapter<'_>,
    writer: &Writer,
) -> Result<(), ConformanceFailure> {
    const STEP: &str = "same_version_twice";
    let steps = chapter.steps;
    let s = scope_ref(&steps, STEP, ScopeKind::Session, "twice-s")?;
    let t = scope_ref(&steps, STEP, ScopeKind::Mob, "twice-t")?;
    let u = scope_ref(&steps, STEP, ScopeKind::Mob, "twice-u")?;
    let base_id = chapter.mint(STEP)?;
    let created = vec![
        create(&s, "base", Vec::new()),
        create(&t, "base", Vec::new()),
        create(&u, "base", Vec::new()),
    ];
    chapter
        .commit(STEP, writer, base_id.clone(), created)
        .await?;
    let x_id = chapter.mint(STEP)?;
    let batch_x = batch(
        &steps,
        STEP,
        x_id.clone(),
        writer,
        vec![
            update(&s, ScopeVersion::FIRST, Some("x"), Vec::new()),
            update(&t, ScopeVersion::FIRST, Some("x"), Vec::new()),
        ],
    )?;
    let y_id = chapter.mint(STEP)?;
    let batch_y = batch(
        &steps,
        STEP,
        y_id.clone(),
        writer,
        vec![
            update(&s, ScopeVersion::FIRST, Some("y"), Vec::new()),
            update(&u, ScopeVersion::FIRST, Some("y"), Vec::new()),
        ],
    )?;
    steps.wrap(STEP, writer.store.commit(&batch_x).await)?;
    let refused = match writer.store.commit(&batch_y).await {
        Ok(()) => false,
        Err(HostStoreError::Conflict { scope, current }) if scope == s && current == Some(V2) => {
            true
        }
        Err(error) => {
            return Err(steps.fail(
                STEP,
                format!("a second batch for one version must land or answer Conflict on {s} at version 2, got {error}"),
            ));
        }
    };

    let witness = chapter.factory.open().await?;
    let mut loads = Vec::new();
    for store in [
        writer.store.as_ref(),
        writer.store.as_ref(),
        witness.as_ref(),
    ] {
        let mut load = Vec::new();
        for scope in [&s, &t, &u] {
            load.push(store.load_head(scope).await);
        }
        loads.push(load);
    }
    steps.ensure(
        STEP,
        loads.windows(2).all(|pair| pair[0] == pair[1]),
        format!("repeated loads and loads from a new handle must agree: {loads:?}"),
    )?;
    if matches!(loads[0][0], Err(HostStoreError::Conflict { .. })) {
        return Ok(());
    }
    let (s_by, t_by, u_by) = (
        written_by(&loads[0][0]),
        written_by(&loads[0][1]),
        written_by(&loads[0][2]),
    );
    if s_by == Some(&x_id) {
        steps.ensure(
            STEP,
            t_by == Some(&x_id) && u_by == Some(&base_id),
            format!("{s} shows the first batch, so {t} shows it too and {u} keeps its first version: {loads:?}"),
        )
    } else if s_by == Some(&y_id) {
        steps.ensure(
            STEP,
            !refused,
            format!("a batch refused at commit never shows, but {s} shows it"),
        )?;
        steps.ensure(
            STEP,
            u_by == Some(&y_id) && t_by == Some(&base_id),
            format!("{s} shows the second batch, so {u} shows it too and {t} keeps its first version: {loads:?}"),
        )
    } else {
        Err(steps.fail(
            STEP,
            format!("{s} must show one of the two batches or load as a typed conflict: {loads:?}"),
        ))
    }
}

fn ensure_retry_answer(
    steps: &Steps,
    step: &'static str,
    answered: &Result<(), HostStoreError>,
) -> Result<(), ConformanceFailure> {
    steps.ensure(
        step,
        matches!(
            answered,
            Ok(()) | Err(HostStoreError::Conflict { .. } | HostStoreError::Invalid(_))
        ),
        format!("a retried commit id answers Ok or a typed refusal, got {answered:?}"),
    )
}

/// A commit id sent again acts once, from the same handle or a new one, and
/// before or after the scope moved on.
async fn retry(chapter: &mut Chapter<'_>, writer: &Writer) -> Result<(), ConformanceFailure> {
    const STEP: &str = "retry";
    let steps = chapter.steps;
    let r = scope_ref(&steps, STEP, ScopeKind::Session, "retry-r")?;
    let created_id = chapter.mint(STEP)?;
    chapter
        .commit(STEP, writer, created_id, vec![create(&r, "r1", Vec::new())])
        .await?;
    let r_id = chapter.mint(STEP)?;
    let batch_r = batch(
        &steps,
        STEP,
        r_id.clone(),
        writer,
        vec![update(
            &r,
            ScopeVersion::FIRST,
            Some("r2"),
            rows(&steps, STEP, &["x", "y"])?,
        )],
    )?;
    steps.wrap(STEP, writer.store.commit(&batch_r).await)?;
    let once = shown(V2, &r_id, "r2", rows(&steps, STEP, &["x", "y"])?);
    ensure_retry_answer(&steps, STEP, &writer.store.commit(&batch_r).await)?;
    ensure_shows(
        &steps,
        STEP,
        writer.store.as_ref(),
        &r,
        &once,
        "a retry from the same handle acts once",
    )
    .await?;
    let witness = chapter.factory.open().await?;
    ensure_retry_answer(&steps, STEP, &witness.commit(&batch_r).await)?;
    ensure_shows(
        &steps,
        STEP,
        witness.as_ref(),
        &r,
        &once,
        "a retry from a new handle acts once",
    )
    .await?;

    let r2_id = chapter.mint(STEP)?;
    chapter
        .commit(
            STEP,
            writer,
            r2_id.clone(),
            vec![update(&r, V2, None, rows(&steps, STEP, &["z"])?)],
        )
        .await?;
    ensure_retry_answer(&steps, STEP, &writer.store.commit(&batch_r).await)?;
    let moved_on = shown(V3, &r2_id, "r2", rows(&steps, STEP, &["x", "y", "z"])?);
    for store in [writer.store.as_ref(), witness.as_ref()] {
        ensure_shows(
            &steps,
            STEP,
            store,
            &r,
            &moved_on,
            "a retry after the scope moved on changes nothing",
        )
        .await?;
    }
    Ok(())
}

async fn list_all(
    steps: &Steps,
    step: &'static str,
    store: &dyn HostStore,
    kind: ScopeKind,
    limit: usize,
) -> Result<Vec<ScopeHead>, ConformanceFailure> {
    let mut listed = Vec::new();
    let mut after: Option<ScopeName> = None;
    for _ in 0..MAX_PAGES {
        let page = steps.wrap(step, store.list_heads(kind, after.as_ref(), limit).await)?;
        steps.ensure(
            step,
            page.heads.len() <= limit,
            format!(
                "a {kind} page holds at most {limit} heads, held {}",
                page.heads.len()
            ),
        )?;
        if page.heads.is_empty() {
            steps.ensure(
                step,
                page.next.is_none(),
                format!(
                    "a {kind} page with no heads ends the listing, next was {:?}",
                    page.next
                ),
            )?;
            return Ok(listed);
        }
        listed.extend(page.heads);
        match page.next {
            None => return Ok(listed),
            Some(next) => after = Some(next),
        }
    }
    Err(steps.fail(
        step,
        format!("the {kind} listing did not end within {MAX_PAGES} pages of {limit}"),
    ))
}

/// Rows keep stable indices through truncation and drops, removal takes
/// effect with its batch, listings page every visible scope exactly once in
/// name order, and a batch at the size limits lands byte-exact.
async fn rows_removal_listing(
    chapter: &mut Chapter<'_>,
    writer: &Writer,
) -> Result<(), ConformanceFailure> {
    rows_and_removal(chapter, writer).await?;
    listing(chapter, writer).await?;
    limits(chapter, writer).await
}

const ROWS_STEP: &str = "rows_removal_listing";

async fn read_from(
    steps: &Steps,
    store: &dyn HostStore,
    scope: &ScopeRef,
    from: u64,
    limit: usize,
) -> Result<Vec<Row>, ConformanceFailure> {
    steps.wrap(
        ROWS_STEP,
        store.read_rows(scope, RowIndex::new(from), limit).await,
    )
}

async fn rows_and_removal(
    chapter: &mut Chapter<'_>,
    writer: &Writer,
) -> Result<(), ConformanceFailure> {
    const STEP: &str = ROWS_STEP;
    let steps = chapter.steps;
    let store = writer.store.as_ref();
    let q = scope_ref(&steps, STEP, ScopeKind::Session, "rows-q")?;
    let all = ["q0", "q1", "q2", "q3", "q4", "q5", "q6", "q7", "q8", "q9"];
    let created_id = chapter.mint(STEP)?;
    chapter
        .commit(
            STEP,
            writer,
            created_id,
            vec![create(&q, "q", rows(&steps, STEP, &all)?)],
        )
        .await?;
    for from in [0_u64, 3, 8, 10, 50] {
        for limit in [4_usize, 10, 100] {
            let read = read_from(&steps, store, &q, from, limit).await?;
            let start = (from as usize).min(all.len());
            let end = start.saturating_add(limit).min(all.len());
            steps.ensure(
                STEP,
                rows_are(&read, &all[start..end]),
                format!(
                    "rows from {from} with limit {limit} must be {:?}, read {} rows",
                    &all[start..end],
                    read.len()
                ),
            )?;
        }
    }

    let truncated_id = chapter.mint(STEP)?;
    let truncate = ScopeUpdate {
        append: rows(&steps, STEP, &["n7"])?,
        truncate_after: Some(RowIndex::new(7)),
        ..change(&q, ScopeVersion::FIRST)
    };
    chapter
        .commit(STEP, writer, truncated_id, vec![truncate])
        .await?;
    let read = read_from(&steps, store, &q, 0, 100).await?;
    let truncated = ["q0", "q1", "q2", "q3", "q4", "q5", "q6", "n7"];
    steps.ensure(
        STEP,
        rows_are(&read, &truncated),
        format!(
            "a truncate after 7 with an append leaves {truncated:?}, read {} rows",
            read.len()
        ),
    )?;

    let dropped_id = chapter.mint(STEP)?;
    let drop_three = ScopeUpdate {
        drop_before: Some(RowIndex::new(3)),
        ..change(&q, V2)
    };
    chapter
        .commit(STEP, writer, dropped_id, vec![drop_three])
        .await?;
    let read = read_from(&steps, store, &q, 3, 100).await?;
    steps.ensure(
        STEP,
        rows_are(&read, &truncated[3..]),
        format!(
            "after a drop before 3 the rows from 3 are {:?}, read {} rows",
            &truncated[3..],
            read.len()
        ),
    )?;
    let below = store.read_rows(&q, RowIndex::new(0), 100).await;
    steps.ensure(
        STEP,
        matches!(below, Err(HostStoreError::Invalid(_))),
        format!(
            "reading from below the first visible row must answer Invalid, got {} rows or {:?}",
            below.as_ref().map_or(0, Vec::len),
            below.as_ref().err()
        ),
    )?;

    let past_end_id = chapter.mint(STEP)?;
    let drop_all = ScopeUpdate {
        drop_before: Some(RowIndex::new(100)),
        ..change(&q, V3)
    };
    chapter
        .commit(STEP, writer, past_end_id, vec![drop_all])
        .await?;
    let read = read_from(&steps, store, &q, 8, 100).await?;
    steps.ensure(
        STEP,
        read.is_empty(),
        format!(
            "a drop past the end removes every row, read {} rows",
            read.len()
        ),
    )?;
    let appended_id = chapter.mint(STEP)?;
    let append = ScopeUpdate {
        append: rows(&steps, STEP, &["p8"])?,
        ..change(&q, ScopeVersion::new(4))
    };
    chapter
        .commit(STEP, writer, appended_id, vec![append])
        .await?;
    let read = read_from(&steps, store, &q, 8, 100).await?;
    steps.ensure(
        STEP,
        rows_are(&read, &["p8"]),
        format!(
            "a drop past the end keeps the end, so the next row takes index 8; read {} rows from 8",
            read.len()
        ),
    )?;

    let head_only_id = chapter.mint(STEP)?;
    chapter
        .commit(
            STEP,
            writer,
            head_only_id.clone(),
            vec![update(&q, ScopeVersion::new(5), Some("moved"), Vec::new())],
        )
        .await?;
    let head = steps.wrap(STEP, store.load_head(&q).await)?;
    let read = read_from(&steps, store, &q, 8, 100).await?;
    steps.ensure(
        STEP,
        head == Some(VersionedHead {
            version: ScopeVersion::new(6),
            commit_id: head_only_id,
            head: b"moved".to_vec(),
        }) && rows_are(&read, &["p8"]),
        format!(
            "a head-only update moves the head and keeps the rows: {head:?}, {} rows from 8",
            read.len()
        ),
    )?;

    let deleted_id = chapter.mint(STEP)?;
    let delete = ScopeUpdate {
        delete: true,
        ..change(&q, ScopeVersion::new(6))
    };
    chapter
        .commit(STEP, writer, deleted_id, vec![delete])
        .await?;
    let gone = Shown {
        head: None,
        rows: Vec::new(),
    };
    ensure_shows(
        &steps,
        STEP,
        store,
        &q,
        &gone,
        "a deleted scope loads nothing and has no rows",
    )
    .await?;
    let sessions = list_all(&steps, STEP, store, ScopeKind::Session, ALL_ROWS).await?;
    steps.ensure(
        STEP,
        sessions.iter().all(|entry| entry.name != q.name),
        format!("a deleted scope is not listed: {q}"),
    )
}

async fn listing(chapter: &mut Chapter<'_>, writer: &Writer) -> Result<(), ConformanceFailure> {
    const STEP: &str = ROWS_STEP;
    const RECORDS: usize = 210;
    const PER_BATCH: usize = 30;
    let steps = chapter.steps;
    let store = writer.store.as_ref();
    let names: Vec<String> = (0..RECORDS)
        .map(|index| format!("list-{index:03}"))
        .collect();
    let mut expected: BTreeMap<String, VersionedHead> = BTreeMap::new();
    for start in (0..RECORDS).step_by(PER_BATCH) {
        let id = chapter.mint(STEP)?;
        let mut created = Vec::with_capacity(PER_BATCH);
        for name in &names[start..(start + PER_BATCH).min(RECORDS)] {
            created.push(create(
                &scope_ref(&steps, STEP, ScopeKind::Record, name)?,
                "v1",
                Vec::new(),
            ));
            expected.insert(
                name.clone(),
                VersionedHead {
                    version: ScopeVersion::FIRST,
                    commit_id: id.clone(),
                    head: b"v1".to_vec(),
                },
            );
        }
        chapter.commit(STEP, writer, id, created).await?;
    }
    let identities = ["identity-0", "identity-1", "identity-2"];
    let identities_id = chapter.mint(STEP)?;
    let mut created = Vec::with_capacity(identities.len());
    for name in identities {
        created.push(create(
            &scope_ref(&steps, STEP, ScopeKind::Identity, name)?,
            "identity",
            Vec::new(),
        ));
    }
    chapter.commit(STEP, writer, identities_id, created).await?;
    let moved_id = chapter.mint(STEP)?;
    let mut moved = Vec::new();
    for (index, name) in names.iter().enumerate() {
        let scope = scope_ref(&steps, STEP, ScopeKind::Record, name)?;
        if index % 21 == 0 {
            moved.push(ScopeUpdate {
                delete: true,
                ..change(&scope, ScopeVersion::FIRST)
            });
            expected.remove(name);
        } else if index % 7 == 3 {
            moved.push(update(&scope, ScopeVersion::FIRST, Some("v2"), Vec::new()));
            expected.insert(
                name.clone(),
                VersionedHead {
                    version: V2,
                    commit_id: moved_id.clone(),
                    head: b"v2".to_vec(),
                },
            );
        }
    }
    chapter.commit(STEP, writer, moved_id, moved).await?;

    let visible: Vec<&str> = expected.keys().map(String::as_str).collect();
    for limit in [16, 1_000] {
        let listed = list_all(&steps, STEP, store, ScopeKind::Record, limit).await?;
        let names_listed: Vec<&str> = listed.iter().map(|entry| entry.name.as_str()).collect();
        steps.ensure(
            STEP,
            names_listed == visible,
            format!("pages of {limit} must list every visible record once, in name order: {names_listed:?}"),
        )?;
        for entry in &listed {
            let loaded = steps.wrap(
                STEP,
                store
                    .load_head(&ScopeRef::new(ScopeKind::Record, entry.name.clone()))
                    .await,
            )?;
            steps.ensure(
                STEP,
                loaded.as_ref() == Some(&entry.head)
                    && expected.get(entry.name.as_str()) == Some(&entry.head),
                format!("the listed head of {} must be its visible head: listed {:?}, loaded {loaded:?}", entry.name, entry.head),
            )?;
        }
    }
    let after = steps.wrap(STEP, ScopeName::try_new("list-100"))?;
    let page = steps.wrap(
        STEP,
        store.list_heads(ScopeKind::Record, Some(&after), 5).await,
    )?;
    let paged: Vec<&str> = page.heads.iter().map(|entry| entry.name.as_str()).collect();
    let following: Vec<&str> = visible
        .iter()
        .copied()
        .filter(|name| *name > "list-100")
        .take(5)
        .collect();
    steps.ensure(
        STEP,
        paged == following,
        format!("the 5 records after list-100 must be {following:?}, listed {paged:?}"),
    )?;
    let listed = list_all(&steps, STEP, store, ScopeKind::Identity, ALL_ROWS).await?;
    let names_listed: Vec<&str> = listed.iter().map(|entry| entry.name.as_str()).collect();
    steps.ensure(
        STEP,
        names_listed == identities,
        format!("the identity listing must be exactly {identities:?}, listed {names_listed:?}"),
    )
}

async fn limits(chapter: &mut Chapter<'_>, writer: &Writer) -> Result<(), ConformanceFailure> {
    const STEP: &str = ROWS_STEP;
    const LIMIT_ROWS: usize = MAX_BATCH_BYTES / MAX_ROW_BYTES;
    let steps = chapter.steps;
    let fill = |index: usize| (index as u8).wrapping_add(1);
    let scope = scope_ref(&steps, STEP, ScopeKind::Session, "limits")?;
    let created_id = chapter.mint(STEP)?;
    chapter
        .commit(
            STEP,
            writer,
            created_id,
            vec![create(&scope, "limits", Vec::new())],
        )
        .await?;
    let append = (0..LIMIT_ROWS)
        .map(|index| steps.wrap(STEP, Row::try_new(vec![fill(index); MAX_ROW_BYTES])))
        .collect::<Result<Vec<_>, _>>()?;
    let full_id = chapter.mint(STEP)?;
    let full = batch(
        &steps,
        STEP,
        full_id,
        writer,
        vec![update(&scope, ScopeVersion::FIRST, None, append)],
    )?;
    steps.ensure(
        STEP,
        full.payload_bytes() == MAX_BATCH_BYTES,
        format!(
            "the limits batch must carry exactly {MAX_BATCH_BYTES} bytes, carries {}",
            full.payload_bytes()
        ),
    )?;
    steps.wrap(STEP, writer.store.commit(&full).await)?;
    drop(full);
    let read = steps.wrap(
        STEP,
        writer
            .store
            .read_rows(&scope, RowIndex::new(0), LIMIT_ROWS)
            .await,
    )?;
    steps.ensure(
        STEP,
        read.len() == LIMIT_ROWS
            && read.iter().enumerate().all(|(index, row)| {
                row.len() == MAX_ROW_BYTES && row.as_bytes().iter().all(|byte| *byte == fill(index))
            }),
        format!(
            "{LIMIT_ROWS} rows of {MAX_ROW_BYTES} bytes must read back byte-exact, read {} rows",
            read.len()
        ),
    )
}

/// Scopes commit concurrently: no realm-wide lock or sequence serializes them
/// into failure or a stall.
#[cfg(not(target_arch = "wasm32"))]
async fn independence(
    chapter: &mut Chapter<'_>,
    writer: &Writer,
) -> Result<(), ConformanceFailure> {
    const STEP: &str = "independence";
    const SCOPES: usize = 210;
    const VERSIONS: u64 = 4;
    let steps = chapter.steps;
    let barrier = Arc::new(tokio::sync::Barrier::new(SCOPES));
    let mut scopes = Vec::with_capacity(SCOPES);
    let mut tasks = Vec::with_capacity(SCOPES);
    for index in 0..SCOPES {
        let scope = scope_ref(
            &steps,
            STEP,
            ScopeKind::Session,
            &format!("independent-{index:03}"),
        )?;
        let mut batches = Vec::new();
        for version in 1..=VERSIONS {
            let appended = rows(&steps, STEP, &[format!("row-{version}").as_str()])?;
            let one = if version == 1 {
                create(&scope, "v1", appended)
            } else {
                update(
                    &scope,
                    ScopeVersion::new(version - 1),
                    Some("next"),
                    appended,
                )
            };
            batches.push(batch(&steps, STEP, chapter.mint(STEP)?, writer, vec![one])?);
        }
        let store = Arc::clone(&writer.store);
        let barrier = Arc::clone(&barrier);
        tasks.push(tokio::spawn(async move {
            barrier.wait().await;
            for batch in &batches {
                store.commit(batch).await?;
            }
            Ok::<(), HostStoreError>(())
        }));
        scopes.push(scope);
    }
    let joined = tokio::time::timeout(Duration::from_mins(2), async {
        let mut failures = Vec::new();
        for task in &mut tasks {
            match task.await {
                Ok(Ok(())) => {}
                Ok(Err(error)) => failures.push(error.to_string()),
                Err(error) => failures.push(format!("a commit task ended abnormally: {error}")),
            }
        }
        failures
    })
    .await;
    let Ok(failures) = joined else {
        for task in &tasks {
            task.abort();
        }
        return Err(steps.fail(
            STEP,
            format!("{SCOPES} scopes did not finish committing concurrently within two minutes"),
        ));
    };
    steps.ensure(
        STEP,
        failures.is_empty(),
        format!("every concurrent commit must land: {failures:?}"),
    )?;
    for scope in &scopes {
        let now = load_shown(&steps, STEP, writer.store.as_ref(), scope).await?;
        steps.ensure(
            STEP,
            now.head.as_ref().map(|head| head.version) == Some(ScopeVersion::new(VERSIONS))
                && now.rows.len() == VERSIONS as usize,
            format!("{scope} must be at version {VERSIONS} with {VERSIONS} rows: {now:?}"),
        )?;
    }
    Ok(())
}

/// A commit held in flight on one scope does not hold up a commit on another.
#[cfg(not(target_arch = "wasm32"))]
async fn stalled_commit_does_not_block(
    chapter: &mut Chapter<'_>,
    writer: &Writer,
) -> Result<(), ConformanceFailure> {
    const STEP: &str = "stalled_commit_does_not_block";
    let steps = chapter.steps;
    let a = scope_ref(&steps, STEP, ScopeKind::Session, "stall-a")?;
    let b = scope_ref(&steps, STEP, ScopeKind::Session, "stall-b")?;
    let a_id = chapter.mint(STEP)?;
    let a_batch = batch(
        &steps,
        STEP,
        a_id.clone(),
        writer,
        vec![create(&a, "a", Vec::new())],
    )?;
    let b_id = chapter.mint(STEP)?;
    let b_batch = batch(
        &steps,
        STEP,
        b_id.clone(),
        writer,
        vec![create(&b, "b", Vec::new())],
    )?;
    let mut held = writer.store.commit(&a_batch);
    let early = poll_up_to(&mut held, 1).await;
    let other = tokio::time::timeout(Duration::from_secs(30), writer.store.commit(&b_batch)).await;
    steps.ensure(
        STEP,
        matches!(other, Ok(Ok(()))),
        format!("a commit on another scope must land while one is held in flight, got {other:?}"),
    )?;
    let finished = if let Some(finished) = early {
        drop(held);
        finished
    } else {
        held.await
    };
    steps.wrap(STEP, finished)?;
    let what = "both commits land";
    let a_shows = shown(ScopeVersion::FIRST, &a_id, "a", Vec::new());
    ensure_shows(&steps, STEP, writer.store.as_ref(), &a, &a_shows, what).await?;
    let b_shows = shown(ScopeVersion::FIRST, &b_id, "b", Vec::new());
    ensure_shows(&steps, STEP, writer.store.as_ref(), &b, &b_shows, what).await
}
