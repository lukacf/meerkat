//! The mob announcement store contract.

use std::collections::{BTreeMap, BTreeSet};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use super::declarations::{
    DeclarationRecordKind, DeclarationRefusal, DeclarationStatus, Declarations, PutRoleTemplate,
    RoleTemplate,
};

use super::availability::SuspendMarker;
use super::escalations::{
    Escalation, EscalationEvent, EscalationNotice, EscalationNoticePage, EscalationPass,
    EscalationSubject, MemberRead, StatusCounts,
};
use super::handling::{BatchHandling, HandlingRead};
use super::sources::{
    AdvanceSource, AnnouncementSource, AuditQuery, AuditRow, MemberFacts, PutSource,
    RecordSourceFailure, RosterFacts, SourceAdvance, SourceChange, SourceQuery, SourceStream,
    UnhandledPolicy,
};
use super::types::{
    AnnounceOutcome, AnnounceRequest, Announcement, AnnouncementStoreError, Batch, BatchEvent,
    BatchId, BatchItem, BatchPosition, BatchQuery, BatchStateTag, ChildMemberLimits, ExpiryReport,
    Member, MemberBacklog, MemberRouteRecord, MobId, MobLineage, OperationId, PruneReport,
    PutSubscription, Result, SealingPolicy, SourceId, SourceKey, Subscription, SubscriptionChange,
    SubscriptionChangeOutcome, SubscriptionEnd, SubscriptionId, SubscriptionState, Topic,
};

/// Durable mob announcements, member subscriptions and delivery batches.
///
/// Every method is one atomic transaction. In particular:
///
/// - `announce` commits the announcement and its capture for every matching
///   subscription together, so a committed announcement is never missing an
///   owed announcement it should have produced, and nothing is captured for
///   an announcement that did not commit.
/// - `seal_batch` re-checks the sealing policy and binds the batch's exact
///   items and immutable body in the same transaction.
/// - `announce` refuses an announcement whose rendering, JSON-encoded,
///   exceeds [`super::types::MAX_ITEM_WIRE_BYTES`], so any one announcement
///   fits a batch for a member on a peer gateway.
/// - `transition_batch` applies [`super::types::BatchState::apply`] against
///   the state the caller observed, and settles or moves the batch's owed
///   announcements in the same transaction.
///
/// Mutations of subscriptions and sources carry an operation id: a replay
/// with the same request returns the original result, and the same id with
/// another request is refused. Implementations must keep those receipts at
/// least as long as the retention window.
///
/// The source methods have default bodies that refuse, so a store written
/// before sources existed still compiles and says it keeps none.
#[async_trait]
pub trait AnnouncementStore: Send + Sync {
    async fn announce(&self, request: AnnounceRequest, now_ms: u64) -> Result<AnnounceOutcome>;

    async fn announcement(
        &self,
        mob_id: &MobId,
        source_key: &SourceKey,
    ) -> Result<Option<Announcement>>;

    async fn put_subscription(
        &self,
        operation: &OperationId,
        request: PutSubscription,
        now_ms: u64,
    ) -> Result<Subscription>;

    async fn change_subscription(
        &self,
        operation: &OperationId,
        id: &SubscriptionId,
        expected_revision: u64,
        change: SubscriptionChange,
        now_ms: u64,
    ) -> Result<SubscriptionChangeOutcome>;

    async fn subscription(&self, id: &SubscriptionId) -> Result<Option<Subscription>>;

    /// End every live subscription of `member` for `end`, a final fact the
    /// caller read from the member's owner: each is marked ended at a new
    /// revision, what it captured and no batch holds settles, and its
    /// sealed batches nothing else wants are cancelled. Returns the ended
    /// subscriptions.
    async fn end_member(
        &self,
        member: &Member,
        end: SubscriptionEnd,
        now_ms: u64,
    ) -> Result<Vec<SubscriptionId>>;

    /// The mobs live subscriptions name, and the mobs whose announcement
    /// log is not marked ended: every mob whose destruction matters here.
    async fn known_mobs(&self) -> Result<BTreeSet<MobId>>;

    /// Drop destroyed mobs from every live subscription that names them, at
    /// a new revision. What was captured from them stays owed. A
    /// subscription left with no mob ends ([`SubscriptionEnd::MobsEnded`]).
    /// Their announcement logs are marked ended, so retention can remove
    /// them once nothing refers to them. Returns the changed subscriptions.
    async fn drop_ended_mobs(
        &self,
        ended: &BTreeSet<MobId>,
        now_ms: u64,
    ) -> Result<Vec<SubscriptionId>>;

    async fn subscriptions(
        &self,
        member: Option<&Member>,
        after: Option<&SubscriptionId>,
        limit: usize,
    ) -> Result<Vec<Subscription>>;

    /// The live lineage records: at most one per mob, for the incarnation
    /// hosted when it was derived.
    async fn lineage(&self) -> Result<Vec<MobLineage>>;

    /// The member's kept route: the route of its first subscription, kept
    /// while it has subscriptions or batches. A put whose route differs is
    /// refused with
    /// [`RouteKept`](super::types::AnnouncementStoreError::RouteKept).
    async fn member_route(&self, member: &Member) -> Result<Option<MemberRouteRecord>>;

    /// End every live subscription of the member, as [`Self::end_member`]
    /// does, and drop its kept route in the same transaction: the mob it
    /// was kept for is gone or was created again. Batches keep the route
    /// they were sealed with.
    async fn unbind_member_route(
        &self,
        member: &Member,
        end: SubscriptionEnd,
        now_ms: u64,
    ) -> Result<Vec<SubscriptionId>>;

    /// Members kept on the route to mobs members created, with their kept
    /// routes.
    async fn child_member_routes(&self) -> Result<Vec<(Member, MemberRouteRecord)>>;

    /// End the member's live subscriptions made before `cursor` of its own
    /// mob's event log, as [`Self::end_member`] ends them.
    async fn end_member_before(
        &self,
        member: &Member,
        cursor: u64,
        end: SubscriptionEnd,
        now_ms: u64,
    ) -> Result<Vec<SubscriptionId>>;

    /// Where the next check of the member's own mob log starts: after the
    /// earliest cursor a live subscription of the member was made at, or
    /// after the cursor a check already read through, whichever is later.
    /// `None` when no live subscription of the member records a cursor.
    async fn member_mob_check_from(&self, member: &Member) -> Result<Option<u64>>;

    /// Record that the member's own mob neither completed nor reset after
    /// [`Self::member_mob_check_from`] through `through`. Only ever
    /// advances.
    async fn record_member_mob_check(&self, member: &Member, through: u64) -> Result<()>;

    /// Record a derived lineage. A live record of the same mob and
    /// incarnation stays as it is: a relation is derived once. A live
    /// record of another incarnation ends first. Returns whether a record
    /// was written.
    async fn record_lineage(&self, lineage: &MobLineage, now_ms: u64) -> Result<bool>;

    /// End the live lineage records of `mobs`: they are no longer hosted.
    async fn end_lineage(&self, mobs: &BTreeSet<MobId>, now_ms: u64) -> Result<()>;

    /// Members with deliverable announcements and no unsettled batch,
    /// oldest first. A member whose batch is outstanding cannot be sealed
    /// again until that batch settles, so it takes no place in the page.
    async fn backlogs(&self, limit: usize) -> Result<Vec<MemberBacklog>>;

    /// [`Self::backlogs`] as a round at `now_ms` reads them: a member that
    /// awaits the handling of its previous batch cannot be sealed until
    /// that batch's turn ends or the wait's bound passes, so it takes no
    /// place in the page either, and the page reads past it. A store that
    /// keeps no handling has no member waiting.
    async fn backlogs_at(&self, limit: usize, now_ms: u64) -> Result<Vec<MemberBacklog>> {
        let _ = now_ms;
        self.backlogs(limit).await
    }

    /// Seal the member's deliverable backlog into one batch if the policy
    /// says it is due at `now_ms`; `None` when it is not. The batch carries
    /// the member's kept route, read in the same transaction; a member
    /// without one is refused with [`AnnouncementStoreError::RouteNotKept`]
    /// and nothing is sealed, since no route is assumed. A batch on a peer
    /// route also stops where its JSON-encoded body, counted with
    /// [`super::types::header_wire_bound`], would exceed one peer request
    /// ([`super::types::MAX_REMOTE_BODY_WIRE_BYTES`]).
    ///
    /// A store that keeps sources seals here with no roster facts, so a
    /// member is covered by member grants alone: a caller whose sources
    /// grant by role or label uses [`Self::seal_batch_with_facts`].
    async fn seal_batch(
        &self,
        member: &Member,
        batch_id: &BatchId,
        policy: &SealingPolicy,
        now_ms: u64,
    ) -> Result<Option<Batch>>;

    /// [`Self::seal_batch`] with the member's roster facts as the owner read
    /// them. A store that keeps sources checks every item it would seal, and
    /// every item it holds, against the member's coverage and routes first:
    /// an item that fails is held, not sealed and not dropped, and a held
    /// item that passes this fresh check is released and sealed. A store
    /// without sources owns no topic and seals as before.
    async fn seal_batch_with_facts(
        &self,
        member: &Member,
        batch_id: &BatchId,
        policy: &SealingPolicy,
        facts: Option<MemberFacts>,
        now_ms: u64,
    ) -> Result<Option<Batch>> {
        let _ = facts;
        self.seal_batch(member, batch_id, policy, now_ms).await
    }

    /// The last check before a native submission of batch `id`, which is in
    /// state `expected`: sealed, or submitting after an answer that nothing
    /// was committed. Every item is checked again against its member's
    /// coverage and routes (`facts`). When all pass, the batch is left as it
    /// is. When one fails and `may_cancel` (nothing native can hold the
    /// batch any more), the batch is cancelled: its failing items are held,
    /// the rest reopen for the member's next batch and its gap lines go to
    /// that batch. Without `may_cancel` (an earlier request to a peer may
    /// still commit) nothing changes and the failure comes back. A store
    /// without sources owns no topic and passes every batch.
    async fn check_before_submission(
        &self,
        id: &BatchId,
        expected: BatchStateTag,
        facts: Option<MemberFacts>,
        may_cancel: bool,
        now_ms: u64,
    ) -> Result<SubmissionCheck> {
        let _ = (id, expected, facts, may_cancel, now_ms);
        Ok(SubmissionCheck::Passed)
    }

    /// Members whose held items are due a fresh check: a hold of the
    /// source's current generation, on an active subscription, last checked
    /// at or before `checked_before_ms`; at most `limit` members, the one
    /// checked longest ago first.
    async fn members_with_holds_due(
        &self,
        checked_before_ms: u64,
        limit: usize,
    ) -> Result<Vec<Member>> {
        let _ = (checked_before_ms, limit);
        Ok(Vec::new())
    }

    /// Check `member`'s held items afresh, at most `limit`, the ones checked
    /// longest ago first: one that passes is released and counted; one that
    /// still fails is stamped checked, so the next check reaches the others.
    async fn release_holds(
        &self,
        member: &Member,
        facts: Option<MemberFacts>,
        limit: u32,
        now_ms: u64,
    ) -> Result<()> {
        let _ = (member, facts, limit, now_ms);
        Ok(())
    }

    /// Count, for batch `id` just admitted into its member's session, each
    /// item its member's coverage or routes (`facts`) no longer allow: it was
    /// committed while they did, and reached the member after they stopped.
    async fn audit_delivered_across_revocation(
        &self,
        id: &BatchId,
        facts: Option<MemberFacts>,
        now_ms: u64,
    ) -> Result<()> {
        let _ = (id, facts, now_ms);
        Ok(())
    }

    async fn batch(&self, id: &BatchId) -> Result<Option<Batch>>;

    async fn batches(&self, query: &BatchQuery) -> Result<Vec<Batch>>;

    /// Batches that are sealed, submitting or committed, in (sealed time,
    /// id) order, starting after `after` when it is given.
    async fn unsettled_batches(
        &self,
        after: Option<&BatchPosition>,
        limit: usize,
    ) -> Result<Vec<Batch>>;

    /// The position of the last unsettled batch in that order, if any.
    async fn newest_unsettled_batch(&self) -> Result<Option<BatchPosition>>;

    async fn transition_batch(
        &self,
        id: &BatchId,
        expected: BatchStateTag,
        event: BatchEvent,
        now_ms: u64,
    ) -> Result<Batch>;

    /// Batches that settled without their delivery being confirmed and
    /// whose next late read is due at `now_ms`, the longest due first. A
    /// batch enters this set when it settles so, and leaves it when
    /// [`Self::record_late_read`] names no next read or retention removes
    /// it.
    async fn late_reads_due(&self, now_ms: u64, limit: usize) -> Result<Vec<Batch>>;

    /// Record a late read of a batch that settled without its delivery being
    /// confirmed: the outcome it found, if any, is kept on the batch, which
    /// is otherwise left as it is (its state, and when it settled), and the
    /// next read is due at `next_read_at_ms`, or never when `None`. Any
    /// other batch is refused.
    async fn record_late_read(
        &self,
        id: &BatchId,
        late: Option<super::types::LateOutcome>,
        next_read_at_ms: Option<u64>,
    ) -> Result<Batch>;

    /// The handling of batch `id` once it was admitted: what became of its
    /// turn, and what was done about it.
    async fn handling(&self, id: &BatchId) -> Result<Option<BatchHandling>> {
        let _ = id;
        Ok(None)
    }

    /// The handling of each of `ids` that was admitted.
    async fn handlings(&self, ids: Vec<BatchId>) -> Result<BTreeMap<BatchId, BatchHandling>> {
        let _ = ids;
        Ok(BTreeMap::new())
    }

    /// Admitted batches whose next handling read is due at `now_ms`, the
    /// longest due first. A batch enters this set when it is admitted, and
    /// leaves it when [`Self::record_handling`] records a final read or
    /// retention removes it.
    async fn handling_reads_due(&self, now_ms: u64, limit: usize) -> Result<Vec<Batch>> {
        let _ = (now_ms, limit);
        Ok(Vec::new())
    }

    /// When the next handling read is due, if any is.
    async fn next_handling_read(&self) -> Result<Option<u64>> {
        Ok(None)
    }

    /// Record one handling read of the admitted batch `id`. A final read
    /// settles the handling, and one that finds the turn unhandled decides
    /// in the same transaction what happens to each item the batch
    /// delivered: a source item follows its source's
    /// `delivery.on_unhandled`, a plain announcement `plain`. Redeliver owes
    /// the item again for the member's next batch, first, under a notice;
    /// past [`super::handling::MAX_REDELIVERIES`], with hold, or for a cause
    /// a replay could mislead on, the item is held for an operator; report
    /// changes nothing. A read of a final handling changes nothing.
    async fn record_handling(
        &self,
        id: &BatchId,
        read: HandlingRead,
        plain: UnhandledPolicy,
        now_ms: u64,
    ) -> Result<BatchHandling> {
        let _ = (id, read, plain, now_ms);
        Err(no_handling())
    }

    /// The notice a successor carrying `items` on renders: a line for each
    /// item the member is owed again after an unhandled turn.
    async fn redelivery_notice(&self, member: &Member, items: &[BatchItem]) -> Result<String> {
        let _ = (member, items);
        Ok(String::new())
    }

    /// The answer an operator's redelivery of batch `id` recorded under
    /// `operation`, if it was made: a retry answers it without reading
    /// anything else. The same operation id with another request is
    /// refused.
    async fn redeliver_receipt(
        &self,
        operation: &OperationId,
        id: &BatchId,
    ) -> Result<Option<BatchHandling>> {
        let _ = (operation, id);
        Ok(None)
    }

    /// An operator's redelivery of what batch `id` holds after its turn
    /// ended unhandled: each held item is checked afresh for coverage with
    /// the member's roster `facts`, and what passes is owed again for the
    /// member's next batch; what fails waits on coverage. Refused
    /// [`AnnouncementStoreError::NotHeld`] for a batch whose handling is not
    /// unhandled with items held.
    async fn redeliver_held(
        &self,
        operation: &OperationId,
        id: &BatchId,
        facts: Option<MemberFacts>,
        now_ms: u64,
    ) -> Result<BatchHandling> {
        let _ = (operation, id, facts, now_ms);
        Err(no_handling())
    }

    /// An operator's dismissal of what batch `id` holds: each held item
    /// settles as handled by an operator. Refused as
    /// [`Self::redeliver_held`] is.
    async fn dismiss_held(
        &self,
        operation: &OperationId,
        id: &BatchId,
        now_ms: u64,
    ) -> Result<BatchHandling> {
        let _ = (operation, id, now_ms);
        Err(no_handling())
    }

    /// Settle as expired what waited longer than the retention window with
    /// nothing native holding it, at most `limit` of each kind: owed
    /// announcements no batch holds (each once per member), and sealed
    /// batches sealed before the window whose last change is older than
    /// `quiet_ms`. A sealed batch may be owed again after a refused
    /// submission while an earlier request for it can still commit, so it
    /// waits out the quiet window after its last request first. The
    /// member's next batch reports them as gaps. A batch the native owner
    /// may hold is never expired here.
    async fn expire(
        &self,
        now_ms: u64,
        retention_ms: u64,
        quiet_ms: u64,
        limit: usize,
    ) -> Result<ExpiryReport>;

    /// Remove what retention allows, oldest first and at most `limit` of
    /// each kind: announcements that no subscription is still owed and that
    /// are older than the window, across mobs, each mob stopping at its
    /// first announcement that is not (so its retained history never has
    /// holes); settled batches no owed announcement still names; old
    /// operation receipts; and cancelled or ended subscriptions that
    /// changed before the window and hold no unsettled batch, with the
    /// member rows nothing refers to and the logs of ended mobs that retain
    /// nothing and that no subscription names; and lineage records that
    /// ended before the window and that no live record names as parent.
    /// [`PruneReport::more`] says whether another pass has work.
    async fn prune(&self, now_ms: u64, retention_ms: u64, limit: usize) -> Result<PruneReport>;

    /// Declare a source, replace its declaration at its revision, or start
    /// its next generation, in one transaction with the claim of every topic
    /// it owns. A source of another live source's principal and scope is
    /// refused ([`AnnouncementStoreError::SourceIdentityTaken`]), and so is
    /// a topic another source owns, live or ended
    /// ([`AnnouncementStoreError::TopicOwned`]).
    async fn put_source(
        &self,
        operation: &OperationId,
        request: PutSource,
        now_ms: u64,
    ) -> Result<AnnouncementSource> {
        let _ = (operation, request, now_ms);
        Err(no_sources())
    }

    /// Pause, resume or end a source at its revision.
    async fn change_source(
        &self,
        operation: &OperationId,
        mob_id: &MobId,
        source_id: &SourceId,
        expected_revision: u64,
        change: SourceChange,
        now_ms: u64,
    ) -> Result<AnnouncementSource> {
        let _ = (
            operation,
            mob_id,
            source_id,
            expected_revision,
            change,
            now_ms,
        );
        Err(no_sources())
    }

    async fn source(
        &self,
        mob_id: &MobId,
        source_id: &SourceId,
    ) -> Result<Option<AnnouncementSource>> {
        let _ = (mob_id, source_id);
        Err(no_sources())
    }

    /// Sources in `(mob_id, source_id)` order.
    async fn sources(&self, query: &SourceQuery) -> Result<Vec<AnnouncementSource>> {
        let _ = query;
        Err(no_sources())
    }

    /// One collector step, in one transaction: the step's receipt, the
    /// fences (state, revision, principal, cursor version), onboarding, each
    /// item announced as the source's (with its captures) or replayed or
    /// recorded as a conflict, at most one gap notice, then the cursor, the
    /// phases and the health. A refusal leaves all of it unchanged; an
    /// onboarding refusal (baseline required, stream not baselined,
    /// backfill bound exceeded) then counts toward the source's failure
    /// streak on its own.
    async fn advance_source(
        &self,
        operation: &OperationId,
        request: AdvanceSource,
        now_ms: u64,
    ) -> Result<SourceAdvance> {
        let _ = (operation, request, now_ms);
        Err(no_sources())
    }

    /// Record a collector's failure, fenced like an advance; the cursor does
    /// not move.
    async fn record_source_failure(
        &self,
        operation: &OperationId,
        request: RecordSourceFailure,
        now_ms: u64,
    ) -> Result<AnnouncementSource> {
        let _ = (operation, request, now_ms);
        Err(no_sources())
    }

    /// The streams the source's current generation onboarded, by name.
    async fn source_streams(
        &self,
        mob_id: &MobId,
        source_id: &SourceId,
    ) -> Result<Vec<SourceStream>> {
        let _ = (mob_id, source_id);
        Err(no_sources())
    }

    /// [`Self::put_subscription`] with the member's roster facts as the owner
    /// read them (`None`: it has none). A topic a source owns admits the
    /// member only if the topic's audience covers it, the member owns no
    /// live conversation on the source's scope, a direct route names the
    /// topic (or the member triages it), and the member would not reach the
    /// source both directly and through a forward. A store that keeps no
    /// sources owns no topic, so its default is the plain put.
    async fn put_subscription_with_facts(
        &self,
        operation: &OperationId,
        request: PutSubscription,
        facts: Option<MemberFacts>,
        now_ms: u64,
    ) -> Result<Subscription> {
        let _ = facts;
        self.put_subscription(operation, request, now_ms).await
    }

    /// [`Self::announce`] with the roster facts of the members whose
    /// subscriptions take a topic a source owns: on such a topic only a
    /// covered member captures, and each withheld capture is an audit row.
    /// A member the facts do not name is covered by a member grant alone.
    async fn announce_with_facts(
        &self,
        request: AnnounceRequest,
        facts: &RosterFacts,
        now_ms: u64,
    ) -> Result<AnnounceOutcome> {
        let _ = facts;
        self.announce(request, now_ms).await
    }

    /// [`Self::advance_source`] with the roster facts of the source's
    /// subscribers, as for [`Self::announce_with_facts`].
    async fn advance_source_with_facts(
        &self,
        operation: &OperationId,
        request: AdvanceSource,
        facts: &RosterFacts,
        now_ms: u64,
    ) -> Result<SourceAdvance> {
        let _ = facts;
        self.advance_source(operation, request, now_ms).await
    }

    /// The members whose live subscriptions take any of `topics` that a
    /// source owns in `mob_id`.
    async fn owned_topic_subscribers(
        &self,
        mob_id: &MobId,
        topics: &BTreeSet<Topic>,
    ) -> Result<BTreeSet<Member>> {
        let _ = (mob_id, topics);
        Ok(BTreeSet::new())
    }

    /// The members whose live subscriptions take any topic `source_id` owns
    /// in `mob_id`.
    async fn source_subscribers(
        &self,
        mob_id: &MobId,
        source_id: &SourceId,
    ) -> Result<BTreeSet<Member>> {
        let _ = (mob_id, source_id);
        Ok(BTreeSet::new())
    }

    /// Audited refusals and withholdings, in id order.
    async fn audit(&self, query: &AuditQuery) -> Result<Vec<AuditRow>> {
        let _ = query;
        Err(no_sources())
    }

    /// One escalation pass, in one transaction: every source and member
    /// condition raised or resolved, open escalations notified again when
    /// due, events past the retention window pruned, and each new raise or
    /// notification announced on the health topic to its audience, with the
    /// roster `facts` of the topic's subscribers. Answers the events it
    /// wrote.
    async fn escalation_pass(
        &self,
        pass: EscalationPass,
        facts: &RosterFacts,
        now_ms: u64,
    ) -> Result<Vec<EscalationNotice>> {
        let _ = (pass, facts, now_ms);
        Err(no_escalations())
    }

    /// The escalation events after `after`, at most `limit`, and the cursor
    /// to read on from.
    async fn escalation_events(
        &self,
        after: u64,
        limit: usize,
    ) -> Result<(Vec<EscalationEvent>, u64)> {
        let _ = (after, limit);
        Err(no_escalations())
    }

    /// The escalation events after `after`, each with its escalation as it
    /// stands: what the owner pushes to the host.
    async fn escalation_notices(&self, after: u64, limit: usize) -> Result<EscalationNoticePage> {
        let _ = (after, limit);
        Err(no_escalations())
    }

    /// The open escalations, of `subject` when one is named.
    async fn open_escalations(
        &self,
        subject: Option<&EscalationSubject>,
    ) -> Result<Vec<Escalation>> {
        let _ = subject;
        Err(no_escalations())
    }

    /// The cursor of the last escalation event; 0 before any.
    async fn last_escalation_cursor(&self) -> Result<u64> {
        Err(no_escalations())
    }

    /// The member's persisted suspend marker.
    async fn member_marker(&self, member: &Member) -> Result<Option<SuspendMarker>> {
        let _ = member;
        Err(no_escalations())
    }

    /// Persist the member's suspend marker, or remove it with `None`.
    async fn put_member_marker(
        &self,
        member: &Member,
        marker: Option<SuspendMarker>,
        now_ms: u64,
    ) -> Result<()> {
        let _ = (member, marker, now_ms);
        Err(no_escalations())
    }

    /// Every persisted suspend marker.
    async fn member_markers(&self) -> Result<Vec<(Member, SuspendMarker)>> {
        Err(no_escalations())
    }

    /// What members/get reads about `member`.
    async fn member_read(&self, member: &Member) -> Result<MemberRead> {
        let _ = member;
        Err(no_escalations())
    }

    /// What members/list reads: members after `after`, at most `limit`,
    /// only those with an open escalation when `escalated`.
    async fn member_reads(
        &self,
        after: Option<&Member>,
        limit: usize,
        escalated: bool,
    ) -> Result<Vec<MemberRead>> {
        let _ = (after, limit, escalated);
        Err(no_escalations())
    }

    /// The members subscribed to the reserved health topic.
    async fn health_subscribers(&self) -> Result<BTreeSet<Member>> {
        Err(no_escalations())
    }

    /// The live subscriptions a source's audience and routes bind: those
    /// that select one of its topics.
    async fn source_bound_subscriptions(
        &self,
        mob_id: &MobId,
        source_id: &SourceId,
    ) -> Result<u32> {
        let _ = (mob_id, source_id);
        Err(no_sources())
    }

    /// meta/status's mob announcement counts at `now_ms`.
    async fn status_counts(&self, now_ms: u64) -> Result<StatusCounts> {
        let _ = now_ms;
        Err(no_escalations())
    }

    /// [`Self::announce_with_facts`] that first makes, in the same
    /// transaction, a child for each member `roles` admits to a live role
    /// template the announcement matches, and captures for no child whose
    /// member left the role. A store that keeps no templates has no
    /// children, so its default is the plain capture.
    async fn announce_with_roles(
        &self,
        request: AnnounceRequest,
        facts: &RosterFacts,
        roles: &RoleExpansions,
        now_ms: u64,
    ) -> Result<AnnounceOutcome> {
        let _ = roles;
        self.announce_with_facts(request, facts, now_ms).await
    }

    /// [`Self::advance_source_with_facts`] with role templates, as for
    /// [`Self::announce_with_roles`], for each item the step announces.
    async fn advance_source_with_roles(
        &self,
        operation: &OperationId,
        request: AdvanceSource,
        facts: &RosterFacts,
        roles: &RoleExpansions,
        now_ms: u64,
    ) -> Result<SourceAdvance> {
        let _ = roles;
        self.advance_source_with_facts(operation, request, facts, now_ms)
            .await
    }

    /// The live role templates, each with its children and their
    /// subscriptions' states: those whose selector names `mob_id`, or every
    /// one.
    async fn role_templates(&self, mob_id: Option<&MobId>) -> Result<Vec<TemplateChildren>> {
        let _ = mob_id;
        Ok(Vec::new())
    }

    async fn role_template(&self, id: &SubscriptionId) -> Result<Option<RoleTemplate>> {
        let _ = id;
        Ok(None)
    }

    /// The live role templates of `mob_id` that select a topic `source_id`
    /// owns there, its forward topics included: those its advances can
    /// capture for.
    async fn source_role_templates(
        &self,
        mob_id: &MobId,
        source_id: &SourceId,
    ) -> Result<Vec<TemplateChildren>> {
        let _ = (mob_id, source_id);
        Ok(Vec::new())
    }

    /// One role template, whatever its state, with its children and their
    /// subscriptions' states.
    async fn role_template_children(
        &self,
        id: &SubscriptionId,
    ) -> Result<Option<TemplateChildren>> {
        let _ = id;
        Ok(None)
    }

    /// Create a role template, or replace its selector at its revision. On a
    /// replace each live child is admitted again: the owner's verdict on its
    /// member in `children`, then the sources' audiences and routes with
    /// `facts`. A child that passes takes the new selector in place, keeping
    /// what it owes; one that fails is pending with the refusal and captures
    /// nothing. The operator's cancel of a child no longer holds.
    async fn put_role_template(
        &self,
        operation: &OperationId,
        request: PutRoleTemplate,
        children: &RoleExpansion,
        facts: &RosterFacts,
        now_ms: u64,
    ) -> Result<RoleTemplate> {
        let _ = (operation, request, children, facts, now_ms);
        Err(no_templates())
    }

    /// Pause, resume or cancel a role template at its revision, and its
    /// children with it, in one transaction: pause pauses every active
    /// child and stops their capture, resume resumes every paused one, and
    /// cancel cancels every live one as a cancel of a subscription does.
    async fn change_role_template(
        &self,
        operation: &OperationId,
        id: &SubscriptionId,
        expected_revision: u64,
        change: SubscriptionChange,
        now_ms: u64,
    ) -> Result<TemplateChangeOutcome> {
        let _ = (operation, id, expected_revision, change, now_ms);
        Err(no_templates())
    }

    /// Cancel a role template at its revision, and every child it made.
    async fn cancel_role_template(
        &self,
        operation: &OperationId,
        id: &SubscriptionId,
        expected_revision: u64,
        now_ms: u64,
    ) -> Result<RoleTemplate> {
        let _ = (operation, id, expected_revision, now_ms);
        Err(no_templates())
    }

    /// Reconcile role children against the members `roles` resolved now: a
    /// member without a live child gets one (or its refusal); a child whose
    /// member left the role is marked left, and captures nothing more; a
    /// child an operator cancelled, or that ended, is held.
    async fn reconcile_role_children(
        &self,
        roles: &RoleExpansions,
        facts: &RosterFacts,
        now_ms: u64,
    ) -> Result<()> {
        let _ = (roles, facts, now_ms);
        Ok(())
    }

    /// Every role child, in template and member order.
    async fn role_children(&self) -> Result<Vec<RoleChild>> {
        Ok(Vec::new())
    }

    /// The declarations last applied, if any.
    async fn applied_declarations(&self) -> Result<Option<AppliedDeclarations>> {
        Ok(None)
    }

    /// Record `applied` as the declarations the reconciler follows.
    async fn record_declarations(&self, applied: AppliedDeclarations) -> Result<()> {
        let _ = applied;
        Err(no_templates())
    }

    /// The marks of the records the declarations manage or refer to, in
    /// kind and id order.
    async fn declaration_marks(&self) -> Result<Vec<DeclarationMark>> {
        Ok(Vec::new())
    }

    /// Write a record's mark. A pending mark raises its
    /// `declaration_pending` escalation; any other resolves it.
    async fn put_declaration_mark(&self, mark: DeclarationMark, now_ms: u64) -> Result<()> {
        let _ = (mark, now_ms);
        Err(no_templates())
    }

    /// Remove a record's mark: the declarations no longer name it, and the
    /// reconciler ended or cancelled what it managed.
    async fn remove_declaration_mark(
        &self,
        kind: DeclarationRecordKind,
        id: &str,
        now_ms: u64,
    ) -> Result<()> {
        let _ = (kind, id, now_ms);
        Err(no_templates())
    }
}

/// What the last check before a submission found.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum SubmissionCheck {
    /// Every item passes.
    Passed,
    /// An item fails, and the batch was left as it is.
    Failed,
    /// An item failed and the batch was cancelled.
    Cancelled(Box<Batch>),
}

fn no_sources() -> AnnouncementStoreError {
    AnnouncementStoreError::Storage("this announcement store keeps no sources".to_string())
}

fn no_handling() -> AnnouncementStoreError {
    AnnouncementStoreError::Storage("this announcement store keeps no handling".to_string())
}

fn no_escalations() -> AnnouncementStoreError {
    AnnouncementStoreError::Storage("this announcement store keeps no escalations".to_string())
}

fn no_templates() -> AnnouncementStoreError {
    AnnouncementStoreError::Storage(
        "this announcement store keeps no declarations or role templates".to_string(),
    )
}

/// The declarations the reconciler follows: the last applied, at its
/// revision and with its digest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppliedDeclarations {
    pub revision: u64,
    pub digest: String,
    pub applied_at_ms: u64,
    pub declarations: Declarations,
}

/// A record the declarations manage, or name and could not make: its
/// declared digest, where it stands and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeclarationMark {
    /// A source or a subscription (a role template is a subscription).
    pub kind: DeclarationRecordKind,
    pub id: String,
    /// The mob of a declared source.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mob_id: Option<MobId>,
    /// Whether the declared subscription is a role template.
    #[serde(default)]
    pub template: bool,
    /// The revision of the declarations that last changed the mark.
    pub revision: u64,
    /// The digest of the declared form the mark was reconciled at.
    pub digest: String,
    pub status: DeclarationStatus,
    /// When the record took this status.
    pub since_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refusal: Option<DeclarationRefusal>,
    /// Whether the reconciler made the runtime record of this id: only such
    /// a record is ever replaced, ended or cancelled.
    pub managed: bool,
}

/// Where a role child stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum RoleChildStatus {
    /// Made for a member with the role: it captures.
    Applied,
    /// A check refused its member: it captures nothing, and is tried again
    /// at each capture and reconcile.
    Pending,
    /// Its member left the role: it stays active, captures nothing more and
    /// delivers what it owes. A member that regains the role captures again
    /// under the same child.
    Left,
    /// An operator cancelled it, or its member's end ended it: held until
    /// the template changes.
    Suppressed,
}

impl RoleChildStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Applied => "applied",
            Self::Pending => "pending",
            Self::Left => "left",
            Self::Suppressed => "suppressed",
        }
    }
}

/// The child a role template made, or tried to make, for one member.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoleChild {
    pub template_id: SubscriptionId,
    pub member: Member,
    /// `{template id}/{member digest, 16 hex}`.
    pub subscription_id: SubscriptionId,
    pub status: RoleChildStatus,
    pub since_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refusal: Option<DeclarationRefusal>,
    /// Paused by its template's pause: its template's resume resumes it. A
    /// child an operator paused by itself is never resumed by its template.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub paused_by_template: bool,
}

/// A role template's change, with its children as they stand after it and
/// the batches that still hold their announcements and have not settled.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TemplateChangeOutcome {
    pub template: RoleTemplate,
    pub children: Vec<Subscription>,
    pub unresolved_batches: Vec<BatchId>,
}

/// What a subscription verb changed: a subscription, or a role template
/// with its children. Each serializes as its own outcome.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(untagged)]
#[non_exhaustive]
pub enum SubscriptionVerbOutcome {
    Subscription(SubscriptionChangeOutcome),
    Template(TemplateChangeOutcome),
}

/// A role template with its children, each with its subscription's state
/// now (`None`: no subscription has its id).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemplateChildren {
    pub template: RoleTemplate,
    pub children: Vec<(RoleChild, Option<SubscriptionState>)>,
}

/// What the owner found for one member that has a template's role now.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum RoleMember {
    /// It has a child already, live or held: nothing to check. A child
    /// marked left captures again.
    Kept,
    /// It passed the owner's checks: a child may be made for it on this
    /// route (a member of a mob a member created, at that mob's event
    /// cursor).
    Admitted {
        route: MemberRouteRecord,
        member_mob_cursor: Option<u64>,
    },
    /// An owner check refused it: its child is pending with the refusal.
    Refused(DeclarationRefusal),
}

/// The members of one role template, as the owner resolved them against
/// the template's revision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoleExpansion {
    pub revision: u64,
    pub members: BTreeMap<Member, RoleMember>,
}

/// The role templates the owner resolved, by id. A template whose roster
/// could not be read now is absent: its children stay as they are, and
/// none is made.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RoleExpansions {
    pub templates: BTreeMap<SubscriptionId, RoleExpansion>,
    /// The bounds on subscriptions of members of mobs members created.
    pub child_limits: ChildMemberLimits,
}
