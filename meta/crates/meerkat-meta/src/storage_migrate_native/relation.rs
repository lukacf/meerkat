//! How a durable session row relates to the committed session a WholeBlob
//! boundary produces.
//!
//! Moved unchanged from the former RuntimeStore facade in `mob_handle_runtime.rs`.
//! The migration's census is their only caller.

use std::sync::Arc;

use meerkat::SessionStore;

/// How a durable head row relates to the committed successor a WholeBlob
/// boundary projects, as the compact head row alone proves it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DurableHeadRelation {
    /// Same rewrite generation, message count and transcript revision.
    AtHead,
    /// Same rewrite generation, and the head row's revision is the committed
    /// transcript's prefix digest at the head's length, which is at least the
    /// last rewrite's result: only trailing appends are missing.
    CommittedAppendPrefix,
    /// Nothing proved from the head row; the durable body decides.
    Unproven,
}

/// The durable row's rewrite generation as its head row records it.
/// `SessionHead::rewrite_count` advances 1:1 with each adopted commit
/// (meerkat-core `session_store.rs`, head CAS). `None` when the store has
/// no incremental channel: such stores load whole blobs whose Session
/// carries its own history, so the caller reads the generation there.
async fn durable_head(
    session_store: &Arc<dyn SessionStore>,
    session_id: &meerkat_core::types::SessionId,
    purpose: &str,
) -> Result<
    Option<meerkat_core::session_store::SessionHead>,
    meerkat_runtime::store::RuntimeStoreError,
> {
    let Some(head_store) = Arc::clone(session_store).as_incremental() else {
        return Ok(None);
    };
    head_store.load_head(session_id).await.map_err(|e| {
        meerkat_runtime::store::RuntimeStoreError::ReadFailed(format!(
            "durable session head read for {purpose}: {e}"
        ))
    })
}

/// How the durable head row relates to the committed successor, decided
/// from the compact head row alone (no durable body read).
///
/// A current-version head stores the transcript content digest the
/// current materialization computes (`head_revision`, the value
/// `transcript_revision()` re-derives and `SessionHead::into_session`
/// verifies fail-closed on every load), its rewrite generation and its
/// message count. That is every fact the body-reading at-head and
/// append-prefix checks consult, so both are proved here without
/// materializing (and re-verifying) the whole durable transcript. Any
/// other head, or no head, is [`DurableHeadRelation::Unproven`] and the
/// caller keeps the full durable read. The committed digests are served
/// from, or seed, the successor's retained midstate and boundary ring, so
/// the projection's later head mint does not hash the transcript again.
pub(super) async fn durable_head_relation(
    session_store: &Arc<dyn SessionStore>,
    successor: &meerkat_core::Session,
    sealed: &meerkat_core::ValidatedTranscriptHistory,
) -> Result<DurableHeadRelation, meerkat_runtime::store::RuntimeStoreError> {
    let Some(head) = durable_head(
        session_store,
        successor.id(),
        "committed->durable projection",
    )
    .await?
    else {
        return Ok(DurableHeadRelation::Unproven);
    };
    if head.version != meerkat_core::SESSION_VERSION {
        return Ok(DurableHeadRelation::Unproven);
    }
    let committed_generation = successor.transcript_rewrite_generation().map_err(|e| {
        meerkat_runtime::store::RuntimeStoreError::WriteFailed(format!(
            "committed rewrite generation before the durable head check: {e}"
        ))
    })?;
    // The digest alone cannot prove the durable row holds the last
    // rewrite (see `durable_is_committed_append_prefix`): a generation
    // mismatch is never proved from the head row.
    if head.rewrite_count != committed_generation {
        return Ok(DurableHeadRelation::Unproven);
    }
    let committed_len = successor.messages().len() as u64;
    if head.message_count == committed_len {
        let committed_revision = successor.transcript_revision().map_err(|e| {
            meerkat_runtime::store::RuntimeStoreError::WriteFailed(format!(
                "committed transcript revision before the durable at-head check: {e}"
            ))
        })?;
        return Ok(if head.head_revision == committed_revision {
            DurableHeadRelation::AtHead
        } else {
            DurableHeadRelation::Unproven
        });
    }
    // The ordinary append boundary: the head row is the committed
    // transcript's own prefix past its last rewrite. Same proof as
    // `durable_is_committed_append_prefix`, with the head row's count and
    // revision standing in for the durable body's.
    let Some(last) = sealed.last_commit() else {
        return Ok(DurableHeadRelation::Unproven);
    };
    let Ok(durable_len) = usize::try_from(head.message_count) else {
        return Ok(DurableHeadRelation::Unproven);
    };
    if durable_len < last.messages_after || head.message_count > committed_len {
        return Ok(DurableHeadRelation::Unproven);
    }
    let committed_prefix = successor
        .transcript_prefix_digest(durable_len)
        .map_err(|e| {
            meerkat_runtime::store::RuntimeStoreError::WriteFailed(format!(
                "committed transcript prefix digest before the durable head check: {e}"
            ))
        })?;
    Ok(if committed_prefix == head.head_revision {
        DurableHeadRelation::CommittedAppendPrefix
    } else {
        DurableHeadRelation::Unproven
    })
}

/// The durable row's rewrite generation as its head row records it (see
/// [`durable_head`]); `None` when the store has no incremental
/// channel, in which case the loaded Session carries its own history.
pub(super) async fn durable_head_generation(
    session_store: &Arc<dyn SessionStore>,
    session_id: &meerkat_core::types::SessionId,
    purpose: &str,
) -> Result<Option<u64>, meerkat_runtime::store::RuntimeStoreError> {
    Ok(durable_head(session_store, session_id, purpose)
        .await?
        .map(|head| head.rewrite_count))
}

/// ORDINARY APPEND BOUNDARY: whether the durable row is the committed
/// transcript's own digest-prefix past its last rewrite.
///
/// The committed successor of an ordinary boundary is the durable row
/// plus this turn's appends, so its current revision differs from the
/// durable row's and the at-head check misses on every boundary that
/// appends rows. The provers then walk the rewrite chain, try both
/// admissions (durable-behind replays the graph per commit), find
/// nothing to repair, and the trailing projection saves the appends -
/// all under the runtime's driver mutex, on every turn (a downstream app, ~3.6 s
/// per pass across 16 identities).
///
/// The proof has two typed parts, no content heuristic:
///
/// - The durable row's rewrite generation equals the committed one. The
///   digest alone cannot prove the durable row holds the last rewrite:
///   the format-2 digest omits row identity and timestamps, so a row a
///   missed rewrite removed and a re-appended row with the same content
///   digest alike. Skipping the walk there would leave the rewrite
///   uninstalled and the projection save refused on every boundary.
/// - The successor's first `d` messages digest-equal the whole durable
///   row (`d` = its length), and `d` is at least the last commit's
///   `messages_after`, so every row the durable row has is the
///   successor's own, past that rewrite's result.
///
/// Only the trailing appends are then missing, which the ordinary
/// projection save lands; its own guard stays the authority on the head.
/// `false` when the seal has no commit (those projections never reach
/// the provers) or the row is longer than the successor.
pub(super) fn durable_is_committed_append_prefix(
    sealed: &meerkat_core::ValidatedTranscriptHistory,
    durable_predecessor: &meerkat_core::Session,
    durable_revision: &str,
    durable_generation: u64,
    committed_generation: u64,
    successor: &meerkat_core::Session,
) -> Result<bool, meerkat_runtime::store::RuntimeStoreError> {
    if durable_generation != committed_generation {
        return Ok(false);
    }
    let Some(last) = sealed.last_commit() else {
        return Ok(false);
    };
    let durable_len = durable_predecessor.messages().len();
    if durable_len < last.messages_after || durable_len > successor.messages().len() {
        return Ok(false);
    }
    let successor_prefix = successor
        .transcript_prefix_digest(durable_len)
        .map_err(|e| {
            meerkat_runtime::store::RuntimeStoreError::WriteFailed(format!(
                "committed transcript prefix digest before rewrite-chain provers: {e}"
            ))
        })?;
    Ok(successor_prefix == durable_revision)
}

/// INVERSE-APPEND ADMISSION (task #56 corpus, a production member field
/// shape): the chain walk accepts a durable predecessor that the commit
/// parent EXTENDS (appends before compaction), but the wedged-retire
/// tear leaves the INVERSE - a durable row extending PAST the sealed
/// parent, because the final appends were projected durably while the
/// retire committed its compaction from the quiesced pre-append state
/// (an unacknowledged boundary is lost whole, by the projection
/// contract). The walk returns `None` on that shape and the repair had
/// no replay path.
///
/// Admission is proof-carrying and exact: the earliest sealed commit
/// whose recorded `parent_revision` equals the digest of the durable
/// row's own first `messages_before` messages proves the durable row is
/// the commit parent plus an unacknowledged suffix; the replay chain is
/// that commit onward, and the exact-parent projection seam truncates
/// the suffix before the typed rewrite replay. No prefix proof means no
/// replay: a durable row from a foreign lineage keeps the injected
/// store's refusal exactly as before.
pub(super) fn inverse_append_replay_chain<'a>(
    sealed: &'a meerkat_core::ValidatedTranscriptHistory,
    durable_predecessor: &meerkat_core::Session,
) -> Result<
    Option<Vec<&'a meerkat_core::TranscriptRewriteCommit>>,
    meerkat_runtime::store::RuntimeStoreError,
> {
    let durable_messages = durable_predecessor.messages();
    let commits: Vec<_> = sealed.state().commits().collect();
    for (index, commit) in commits.iter().enumerate() {
        if commit.messages_before > durable_messages.len() {
            continue;
        }
        let prefix_digest =
            meerkat_core::transcript_messages_digest(&durable_messages[..commit.messages_before])
                .map_err(|e| {
                meerkat_runtime::store::RuntimeStoreError::WriteFailed(format!(
                    "durable prefix digest for inverse-append admission at generation {}: {e}",
                    commit.rewrite_generation
                ))
            })?;
        // The verdict logs UNCONDITIONALLY, matched or not: a silent
        // refusal in the field is indistinguishable from the admission
        // never running (iteration-4 lesson).
        let matched = prefix_digest == commit.parent_revision;
        tracing::info!(
            session_id = %durable_predecessor.id(),
            rewrite_generation = commit.rewrite_generation,
            messages_before = commit.messages_before,
            durable_messages = durable_messages.len(),
            %prefix_digest,
            parent_revision = %commit.parent_revision,
            matched,
            "inverse-append proof verdict"
        );
        if matched {
            return Ok(Some(commits[index..].to_vec()));
        }
    }
    Ok(None)
}

/// DURABLE-BEHIND ADMISSION (task #56 corpus iteration 5, a production
/// member's true field shape; admission authored downstream and landed
/// here with the lane's conventions): the durable row is a strict
/// digest-PREFIX of a committed rewrite's parent - it lacks the parent's
/// last rows (the exact inverse of the inverse-append shape). The shape
/// says nothing about its cause: a projection that failed before the
/// rewrite leaves it, and so do appends inside one committed boundary
/// before a compaction, with no failure at all. The walk proves the
/// earliest such rewrite and returns the chain from it to the sealed
/// head, so a row several rewrites behind replays every one of them.
/// Proof is exact content:
/// the materialized parent's first `durable_len` messages must
/// digest-equal the WHOLE durable row. On proof the existing replay
/// loop does the rest: durable head != parent revision routes through
/// the exact-parent projection seam (bringing durable UP to the parent
/// from the parent's own body), then the commit replays and the
/// trailing projection lands the compacted head. No durable suffix
/// exists to preserve - durable holds no unique content. Without proof,
/// foreign lineage keeps the typed refusal.
pub(super) fn durable_behind_prefix_chain<'a>(
    successor: &meerkat_core::Session,
    sealed: &'a meerkat_core::ValidatedTranscriptHistory,
    durable_predecessor: &meerkat_core::Session,
) -> Result<
    Option<Vec<&'a meerkat_core::TranscriptRewriteCommit>>,
    meerkat_runtime::store::RuntimeStoreError,
> {
    let durable_messages = durable_predecessor.messages();
    let durable_digest =
        meerkat_core::transcript_messages_digest(durable_messages).map_err(|e| {
            meerkat_runtime::store::RuntimeStoreError::WriteFailed(format!(
                "durable digest for durable-behind admission: {e}"
            ))
        })?;
    let commits: Vec<_> = sealed.state().commits().collect();
    for (index, commit) in commits.iter().enumerate() {
        if commit.messages_before < durable_messages.len() {
            continue;
        }
        let parent_session =
            match successor.with_validated_transcript_rewrite_parent_projection(sealed, commit) {
                Ok(parent_session) => parent_session,
                // meerkat 0.8.51 bounds the history a session graph retains:
                // an older rewrite keeps its commit and digests but not its
                // body, so its parent cannot be projected. With no body it can
                // never prove the durable row is its prefix; a later, retained
                // commit still can.
                Err(meerkat_core::TranscriptEditError::TranscriptRevisionRetired { .. }) => {
                    continue;
                }
                Err(e) => {
                    return Err(meerkat_runtime::store::RuntimeStoreError::WriteFailed(
                        format!(
                            "parent projection for durable-behind admission at generation {}: {e}",
                            commit.rewrite_generation
                        ),
                    ));
                }
            };
        let parent_messages = parent_session.messages();
        if parent_messages.len() < durable_messages.len() {
            continue;
        }
        let parent_prefix_digest =
            meerkat_core::transcript_messages_digest(&parent_messages[..durable_messages.len()])
                .map_err(|e| {
                    meerkat_runtime::store::RuntimeStoreError::WriteFailed(format!(
                        "parent prefix digest for durable-behind admission at generation {}: {e}",
                        commit.rewrite_generation
                    ))
                })?;
        let matched = parent_prefix_digest == durable_digest;
        tracing::info!(
            session_id = %durable_predecessor.id(),
            rewrite_generation = commit.rewrite_generation,
            messages_before = commit.messages_before,
            durable_messages = durable_messages.len(),
            parent_messages = parent_messages.len(),
            %parent_prefix_digest,
            %durable_digest,
            matched,
            "durable-behind proof verdict"
        );
        if matched {
            return Ok(Some(commits[index..].to_vec()));
        }
    }
    Ok(None)
}

#[cfg(test)]
#[allow(clippy::panic)]
pub(super) mod tests {
    /// Compact `session[0..end]` into one summary message as a committed
    /// transcript rewrite.
    pub(in crate::storage_migrate_native) fn commit_test_compaction(
        session: &mut meerkat_core::Session,
        end: usize,
        summary: &str,
    ) {
        let parent_revision = session
            .transcript_revision()
            .unwrap_or_else(|error| panic!("{error}"));
        session
            .commit_transcript_rewrite(
                meerkat_core::TranscriptRewriteSelection::MessageRange { start: 0, end },
                vec![meerkat_core::Message::User(
                    meerkat_core::types::UserMessage::text(summary),
                )],
                meerkat_core::TranscriptRewriteReason::new("wedged-turn retire compaction"),
                Some(format!("durable-behind-{summary}")),
                Some(parent_revision),
            )
            .unwrap_or_else(|error| panic!("{error}"));
    }

    /// Direct proof test for the durable-behind admission (independent of
    /// the chain walk's own acceptance behavior): a durable row that is a
    /// strict digest-prefix of the sealed parent proves the chain; a
    /// same-length divergent row proves nothing.
    #[tokio::test]
    async fn durable_behind_prefix_chain_proves_exact_prefix_and_refuses_divergence() {
        let mut base = meerkat_core::Session::new();
        for text in ["opening", "second"] {
            base.push(meerkat_core::Message::User(
                meerkat_core::types::UserMessage::text(text),
            ));
        }
        let durable = base.clone();
        let mut divergent = base.clone();
        divergent.push(meerkat_core::Message::User(
            meerkat_core::types::UserMessage::text("divergent third"),
        ));
        let mut successor = base;
        successor.push(meerkat_core::Message::User(
            meerkat_core::types::UserMessage::text("third"),
        ));
        let parent_revision = successor
            .transcript_revision()
            .unwrap_or_else(|error| panic!("{error}"));
        successor
            .commit_transcript_rewrite(
                meerkat_core::TranscriptRewriteSelection::MessageRange { start: 0, end: 3 },
                vec![meerkat_core::Message::User(
                    meerkat_core::types::UserMessage::text("compacted summary"),
                )],
                meerkat_core::TranscriptRewriteReason::new("wedged-turn retire compaction"),
                Some("task-56-behind-proof-unit".to_string()),
                Some(parent_revision),
            )
            .unwrap_or_else(|error| panic!("{error}"));
        let sealed = successor
            .validated_transcript_history_state()
            .unwrap_or_else(|error| panic!("{error}"))
            .unwrap_or_else(|| panic!("the successor carries a sealed graph"));

        let proven = crate::storage_migrate_native::relation::durable_behind_prefix_chain(
            &successor, &sealed, &durable,
        )
        .unwrap_or_else(|error| panic!("{error}"))
        .unwrap_or_else(|| panic!("a strict prefix of the parent must prove the chain"));
        assert_eq!(proven.len(), 1);
        assert_eq!(proven[0].rewrite_generation, 1);

        let refused = crate::storage_migrate_native::relation::durable_behind_prefix_chain(
            &successor, &sealed, &divergent,
        )
        .unwrap_or_else(|error| panic!("{error}"));
        assert!(
            refused.is_none(),
            "a divergent row must not prove the durable-behind chain"
        );
    }

    /// The durable-behind proof covers the whole chain: a durable row that
    /// is a strict digest-prefix of the FIRST rewrite's parent, with a second
    /// rewrite after it, proves the chain from that rewrite to the sealed
    /// head, so both committed rewrites replay.
    #[tokio::test]
    async fn durable_behind_prefix_chain_spans_every_rewrite_after_the_proving_parent() {
        let mut base = meerkat_core::Session::new();
        for text in ["opening", "second"] {
            base.push(meerkat_core::Message::User(
                meerkat_core::types::UserMessage::text(text),
            ));
        }
        let durable = base.clone();
        let mut successor = base;
        successor.push(meerkat_core::Message::User(
            meerkat_core::types::UserMessage::text("third"),
        ));
        commit_test_compaction(&mut successor, 3, "first summary");
        successor.push(meerkat_core::Message::User(
            meerkat_core::types::UserMessage::text("after the first compaction"),
        ));
        commit_test_compaction(&mut successor, 2, "second summary");
        let sealed = successor
            .validated_transcript_history_state()
            .unwrap_or_else(|error| panic!("{error}"))
            .unwrap_or_else(|| panic!("the successor carries a sealed graph"));

        let proven = crate::storage_migrate_native::relation::durable_behind_prefix_chain(
            &successor, &sealed, &durable,
        )
        .unwrap_or_else(|error| panic!("{error}"))
        .unwrap_or_else(|| panic!("a strict prefix of the first parent must prove the chain"));
        assert_eq!(
            proven
                .iter()
                .map(|commit| commit.rewrite_generation)
                .collect::<Vec<_>>(),
            vec![1, 2]
        );
    }
}
