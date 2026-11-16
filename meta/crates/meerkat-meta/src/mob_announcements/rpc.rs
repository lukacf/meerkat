//! The `meta/mob_announcements/*` JSON-RPC methods.
//!
//! They answer only when the host enabled mob announcements at startup.
//! Parameters are strict: unknown fields are refused. A host call announces
//! as the host; members announce through their own tools.

use std::collections::BTreeSet;
use std::future::Future;

use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use super::declarations::{DeclarationRecordKind, Declarations, PutRoleTemplate, RoleSelector};
use super::escalations::{EscalationSubject, MemberRead};
use super::handling::BatchHandling;
use super::owner::{AnnouncementError, AnnouncementService, HiddenMember};
use super::sources::{
    AdvanceSource, AnnouncementSource, AudienceGrant, AuditAction, AuditQuery, AuditRow, Coverage,
    ObservationCapability, Observed, Principal, PutSource, Rebaseline, RecordSourceFailure,
    SourceChange, SourceConfig, SourceConnection, SourceDelivery, SourceFailure,
    SourceHealthPolicy, SourceIdentity, SourceItem, SourcePage, SourceQuery, SourceRoute,
    SourceRunner, SourceStart, SourceState, StreamOnboarding,
};
use super::store::SubscriptionVerbOutcome;
use super::types::{
    AnnounceRequest, Announcement, AnnouncementRef, AnnouncementStoreError, Announcer, Batch,
    BatchId, BatchQuery, ChildMemberLimits, MAX_ID_BYTES, MAX_PAGE, Member, MemberIdentity, MobId,
    OperationId, PutSubscription, Selector, SourceId, SourceKey, StartFrontier, Subscription,
    SubscriptionChange, SubscriptionDelivery, SubscriptionId, Topic,
};
use crate::access::AccessView;
use crate::http_console::ConsoleAbsentNames;
use crate::rpc::JsonRpcError;

/// Mob announcements are not enabled on this host.
pub const ANNOUNCEMENTS_UNAVAILABLE_CODE: i64 = -32044;
/// A conflicting or stale request: other content under a source key, a
/// stale revision, a reused operation id, a batch that moved.
pub const ANNOUNCEMENTS_CONFLICT_CODE: i64 = -32045;
/// A requested start frontier lies outside a mob's announcements: before
/// what is retained, or beyond its head.
pub const ANNOUNCEMENTS_FRONTIER_CODE: i64 = -32046;
/// A typed refusal: governed runtime, unknown, unsupported or unusable
/// member, unusable peer gateway, cancelled or ended subscription, member
/// change, a native read unavailable now, a change on a candidate launch.
pub const ANNOUNCEMENTS_REFUSED_CODE: i64 = -32047;
/// The named subscription, delivery or mob does not exist here.
pub const ANNOUNCEMENTS_NOT_FOUND_CODE: i64 = -32048;

pub const ANNOUNCEMENT_METHODS: &[&str] = &[
    "meta/mob_announcements/announce",
    "meta/mob_announcements/get",
    "meta/mob_announcements/subscriptions/put",
    "meta/mob_announcements/subscriptions/pause",
    "meta/mob_announcements/subscriptions/resume",
    "meta/mob_announcements/subscriptions/cancel",
    "meta/mob_announcements/subscriptions/get",
    "meta/mob_announcements/subscriptions/list",
    "meta/mob_announcements/deliveries/list",
    "meta/mob_announcements/deliveries/observe",
    "meta/mob_announcements/deliveries/reconcile",
    "meta/mob_announcements/deliveries/redeliver",
    "meta/mob_announcements/deliveries/dismiss",
    "meta/mob_announcements/sources/put",
    "meta/mob_announcements/sources/pause",
    "meta/mob_announcements/sources/resume",
    "meta/mob_announcements/sources/end",
    "meta/mob_announcements/sources/get",
    "meta/mob_announcements/sources/list",
    "meta/mob_announcements/sources/advance",
    "meta/mob_announcements/sources/record_failure",
    "meta/mob_announcements/audit/list",
    "meta/mob_announcements/declarations/apply",
    "meta/mob_announcements/declarations/get",
    "meta/mob_announcements/members/get",
    "meta/mob_announcements/members/list",
    "meta/mob_announcements/escalations/list",
];

/// The feature capability a gateway with announcement sources advertises.
pub const SOURCES_FEATURE_CAPABILITY: &str = "mob_announcements.sources.v1";

/// The feature capabilities a gateway with mob announcements advertises.
pub fn announcement_feature_capabilities() -> Vec<&'static str> {
    vec![SOURCES_FEATURE_CAPABILITY]
}

/// meta/status's `mob_announcements` object: sources by state, members
/// suspended, and open escalations with the last event cursor. A store that
/// cannot be read answers null.
pub async fn status_projection(service: &AnnouncementService) -> Value {
    match service.status_counts().await {
        Ok(counts) => json!({
            "sources": {
                "active": counts.sources_active,
                "paused": counts.sources_paused,
                "failing": counts.sources_failing,
                "stale": counts.sources_stale,
            },
            "members": {
                "suspended": counts.members_suspended,
                "unhandled_held": counts.unhandled_held,
            },
            "handling": {
                "pending": counts.handling_pending,
                "handled": counts.handling_handled,
                "handled_unverified": counts.handling_handled_unverified,
                "unhandled": counts.handling_unhandled,
            },
            "escalations": {
                "open": counts.open_escalations,
                "last_event_cursor": counts.last_event_cursor,
            },
        }),
        Err(error) => {
            tracing::warn!(%error, "mob announcement status not read");
            Value::Null
        }
    }
}

/// meta/capabilities's `mob_announcements` object; null when disabled.
pub async fn capabilities_projection(service: Option<&AnnouncementService>) -> Value {
    let Some(service) = service else {
        return Value::Null;
    };
    json!({
        "sources": "v1",
        "local_delivery": service.local_delivery(),
        "governed_admission": false,
    })
}

/// A source as an answer carries it, with what its health reads at read
/// time: whether quiet and empty coverage detection are configured, its
/// open escalations, and whether this gateway schedules its runner.
async fn with_health(
    service: &AnnouncementService,
    source: &AnnouncementSource,
    mut value: Value,
) -> Result<Value, JsonRpcError> {
    let escalations = service
        .open_escalations(Some(&EscalationSubject::Source {
            mob_id: source.mob_id.clone(),
            source_id: source.source_id.clone(),
        }))
        .await
        .map_err(rpc_error)?;
    let policy = &source.health_policy;
    let health = &source.health;
    if let Some(projection) = value.get_mut("health").and_then(Value::as_object_mut) {
        projection.insert(
            "status".to_string(),
            to_value(service.health_status(source))?,
        );
        projection.insert(
            "quiet_monitoring".to_string(),
            json!({
                "configured": policy.quiet_after_ms.is_some(),
                "quiet_after_ms": policy.quiet_after_ms,
                "last_new_item_at_ms": health.last_new_item_at_ms,
            }),
        );
        projection.insert(
            "empty_coverage_monitoring".to_string(),
            json!({
                "configured": policy.empty_after_ms.is_some(),
                "empty_after_ms": policy.empty_after_ms,
                "empty_snapshot_since_ms": health.empty_snapshot_since_ms,
            }),
        );
        projection.insert("escalations".to_string(), to_value(escalations)?);
        projection.insert(
            "runner_scheduled_here".to_string(),
            source
                .runner
                .as_ref()
                .map_or(Value::Null, |_| Value::Bool(service.runners_installed())),
        );
    }
    Ok(value)
}

pub fn is_announcement_method(method: &str) -> bool {
    ANNOUNCEMENT_METHODS.contains(&method)
}

/// The announcement methods that change state; every other one only reads.
/// A read-only console refuses these.
pub const ANNOUNCEMENT_MUTATE_METHODS: &[&str] = &[
    "meta/mob_announcements/announce",
    "meta/mob_announcements/subscriptions/put",
    "meta/mob_announcements/subscriptions/pause",
    "meta/mob_announcements/subscriptions/resume",
    "meta/mob_announcements/subscriptions/cancel",
    "meta/mob_announcements/deliveries/reconcile",
    "meta/mob_announcements/deliveries/redeliver",
    "meta/mob_announcements/deliveries/dismiss",
    "meta/mob_announcements/sources/put",
    "meta/mob_announcements/sources/pause",
    "meta/mob_announcements/sources/resume",
    "meta/mob_announcements/sources/end",
    "meta/mob_announcements/sources/advance",
    "meta/mob_announcements/sources/record_failure",
    "meta/mob_announcements/declarations/apply",
];

pub fn is_announcement_mutating_method(method: &str) -> bool {
    ANNOUNCEMENT_MUTATE_METHODS.contains(&method)
}

fn invalid_params(message: impl std::fmt::Display) -> JsonRpcError {
    JsonRpcError::new(-32602, format!("Invalid params: {message}"))
}

fn typed(code: i64, kind: &str, message: impl std::fmt::Display, extra: Value) -> JsonRpcError {
    let mut data = json!({ "kind": kind });
    if let (Some(data), Value::Object(extra)) = (data.as_object_mut(), extra) {
        data.extend(extra);
    }
    JsonRpcError::new(code, message.to_string()).with_data(data)
}

pub fn announcements_unavailable_error() -> JsonRpcError {
    typed(
        ANNOUNCEMENTS_UNAVAILABLE_CODE,
        "mob_announcements_unavailable",
        "mob announcements are not enabled on this host",
        Value::Null,
    )
}

/// The typed kind `error` carries on the wire: what a declared record or a
/// role child is pending with when the same refusal meets it there.
pub(super) fn refusal_kind(error: &AnnouncementError) -> String {
    let refused = rpc_error(error.clone());
    match refused.data.as_ref().and_then(|data| data.get("kind")) {
        Some(Value::String(kind)) => kind.clone(),
        _ if refused.code == -32602 => "invalid".to_string(),
        _ => "failed".to_string(),
    }
}

fn rpc_error(error: AnnouncementError) -> JsonRpcError {
    let message = error.to_string();
    match error {
        AnnouncementError::GovernedRuntime => typed(
            ANNOUNCEMENTS_REFUSED_CODE,
            "governed_runtime",
            message,
            Value::Null,
        ),
        AnnouncementError::UnknownMember(member) => typed(
            ANNOUNCEMENTS_REFUSED_CODE,
            "unknown_member",
            message,
            json!({ "member": member }),
        ),
        AnnouncementError::InvalidMember(_) => invalid_params(message),
        AnnouncementError::Configuration(_) => invalid_params(message),
        AnnouncementError::UnusableMember(_) => typed(
            ANNOUNCEMENTS_REFUSED_CODE,
            "unusable_member",
            message,
            Value::Null,
        ),
        AnnouncementError::UnknownMobs(mobs) => typed(
            ANNOUNCEMENTS_NOT_FOUND_CODE,
            "unknown_mob",
            message,
            json!({ "mobs": mobs }),
        ),
        AnnouncementError::UnusablePeer(_) => typed(
            ANNOUNCEMENTS_REFUSED_CODE,
            "unusable_peer",
            message,
            Value::Null,
        ),
        AnnouncementError::NativeUnavailable(_) => typed(
            ANNOUNCEMENTS_REFUSED_CODE,
            "native_unavailable",
            message,
            Value::Null,
        ),
        AnnouncementError::UnrelatedMob(mob_id) => typed(
            ANNOUNCEMENTS_REFUSED_CODE,
            "unrelated_mob",
            message,
            json!({ "mob_id": mob_id }),
        ),
        AnnouncementError::AmbiguousMob(mob_id) => typed(
            ANNOUNCEMENTS_REFUSED_CODE,
            "ambiguous_mob",
            message,
            json!({ "mob_id": mob_id }),
        ),
        AnnouncementError::CandidateLaunch => typed(
            ANNOUNCEMENTS_REFUSED_CODE,
            "candidate_launch",
            message,
            Value::Null,
        ),
        AnnouncementError::Store(error) => match error {
            AnnouncementStoreError::AnnouncementConflict { existing } => typed(
                ANNOUNCEMENTS_CONFLICT_CODE,
                "announcement_conflict",
                message,
                json!({ "existing": *existing }),
            ),
            AnnouncementStoreError::StaleRevision { current } => typed(
                ANNOUNCEMENTS_CONFLICT_CODE,
                "stale_revision",
                message,
                json!({ "current_revision": current }),
            ),
            AnnouncementStoreError::OperationReused => typed(
                ANNOUNCEMENTS_CONFLICT_CODE,
                "operation_reused",
                message,
                Value::Null,
            ),
            AnnouncementStoreError::BatchMoved { current } => typed(
                ANNOUNCEMENTS_CONFLICT_CODE,
                "batch_moved",
                message,
                json!({ "current": current }),
            ),
            AnnouncementStoreError::Gap {
                mob_id,
                earliest_retained,
            } => typed(
                ANNOUNCEMENTS_FRONTIER_CODE,
                "gap",
                message,
                json!({ "mob_id": mob_id, "earliest_retained": earliest_retained }),
            ),
            AnnouncementStoreError::FrontierBeyondHead { mob_id, head } => typed(
                ANNOUNCEMENTS_FRONTIER_CODE,
                "frontier_beyond_head",
                message,
                json!({ "mob_id": mob_id, "head": head }),
            ),
            AnnouncementStoreError::UnknownSubscription => typed(
                ANNOUNCEMENTS_NOT_FOUND_CODE,
                "unknown_subscription",
                message,
                Value::Null,
            ),
            AnnouncementStoreError::UnknownBatch => typed(
                ANNOUNCEMENTS_NOT_FOUND_CODE,
                "unknown_delivery",
                message,
                Value::Null,
            ),
            AnnouncementStoreError::SubscriptionCancelled => typed(
                ANNOUNCEMENTS_REFUSED_CODE,
                "subscription_cancelled",
                message,
                Value::Null,
            ),
            AnnouncementStoreError::SubscriptionEnded { end } => typed(
                ANNOUNCEMENTS_REFUSED_CODE,
                "subscription_ended",
                message,
                json!({ "end": end }),
            ),
            AnnouncementStoreError::MemberMismatch => typed(
                ANNOUNCEMENTS_REFUSED_CODE,
                "member_mismatch",
                message,
                Value::Null,
            ),
            AnnouncementStoreError::RouteKept { kept } => typed(
                ANNOUNCEMENTS_REFUSED_CODE,
                "ambiguous_mob",
                message,
                json!({ "kept_route": kept }),
            ),
            AnnouncementStoreError::ChildMemberLimit { limit, mob_id } => typed(
                ANNOUNCEMENTS_REFUSED_CODE,
                "child_member_limit",
                message,
                json!({ "limit": limit, "mob_id": mob_id }),
            ),
            AnnouncementStoreError::UnknownSource => typed(
                ANNOUNCEMENTS_NOT_FOUND_CODE,
                "unknown_source",
                message,
                Value::Null,
            ),
            AnnouncementStoreError::SourceEnded => typed(
                ANNOUNCEMENTS_REFUSED_CODE,
                "source_ended",
                message,
                Value::Null,
            ),
            AnnouncementStoreError::SourcePaused => typed(
                ANNOUNCEMENTS_REFUSED_CODE,
                "source_paused",
                message,
                Value::Null,
            ),
            AnnouncementStoreError::SourceIdentityTaken { source_id } => typed(
                ANNOUNCEMENTS_REFUSED_CODE,
                "source_identity_taken",
                message,
                json!({ "source_id": source_id }),
            ),
            AnnouncementStoreError::IdentityImmutable => typed(
                ANNOUNCEMENTS_REFUSED_CODE,
                "identity_immutable",
                message,
                Value::Null,
            ),
            AnnouncementStoreError::TopicOwned { topic, source_id } => typed(
                ANNOUNCEMENTS_REFUSED_CODE,
                "topic_owned",
                message,
                json!({ "topic": topic, "source_id": source_id }),
            ),
            AnnouncementStoreError::TopicOwnedBySource { topic, source_id } => typed(
                ANNOUNCEMENTS_REFUSED_CODE,
                "topic_owned_by_source",
                message,
                json!({ "topic": topic, "source_id": source_id }),
            ),
            AnnouncementStoreError::PrincipalNotAllowed { kind } => typed(
                ANNOUNCEMENTS_REFUSED_CODE,
                "principal_not_allowed",
                message,
                json!({ "principal_kind": kind }),
            ),
            AnnouncementStoreError::RouteSelectorUnsupported { selector } => typed(
                ANNOUNCEMENTS_REFUSED_CODE,
                "route_selector_unsupported",
                message,
                json!({ "selector": selector }),
            ),
            AnnouncementStoreError::SourceStaleRevision {
                revision,
                generation,
                cursor,
            } => typed(
                ANNOUNCEMENTS_CONFLICT_CODE,
                "stale_revision",
                message,
                json!({
                    "current_revision": revision,
                    "current_version": cursor.version,
                    "generation": generation,
                    "cursor": cursor,
                }),
            ),
            AnnouncementStoreError::SourceLimit { limit, mob_id } => typed(
                ANNOUNCEMENTS_REFUSED_CODE,
                "source_limit",
                message,
                json!({ "limit": limit, "mob_id": mob_id }),
            ),
            AnnouncementStoreError::SourceVersionConflict {
                revision,
                state,
                cursor,
            } => typed(
                ANNOUNCEMENTS_CONFLICT_CODE,
                "source_version_conflict",
                message,
                json!({
                    "current_version": cursor.version,
                    "cursor": cursor,
                    "state": state,
                    "revision": revision,
                }),
            ),
            AnnouncementStoreError::PrincipalChanged {
                pinned_digest,
                offered_digest,
            } => typed(
                ANNOUNCEMENTS_REFUSED_CODE,
                "principal_changed",
                message,
                json!({ "pinned_digest": pinned_digest, "offered_digest": offered_digest }),
            ),
            AnnouncementStoreError::BaselineRequired => typed(
                ANNOUNCEMENTS_REFUSED_CODE,
                "baseline_required",
                message,
                Value::Null,
            ),
            AnnouncementStoreError::StreamNotBaselined { streams } => typed(
                ANNOUNCEMENTS_REFUSED_CODE,
                "stream_not_baselined",
                message,
                json!({ "streams": streams }),
            ),
            AnnouncementStoreError::BackfillBoundExceeded { remaining, stream } => {
                let mut data = json!({ "remaining": remaining });
                if let Some(stream) = stream {
                    data["stream"] = json!(stream);
                }
                typed(
                    ANNOUNCEMENTS_REFUSED_CODE,
                    "backfill_bound_exceeded",
                    message,
                    data,
                )
            }
            AnnouncementStoreError::DeletionsUnobservable => typed(
                ANNOUNCEMENTS_REFUSED_CODE,
                "deletions_unobservable",
                message,
                Value::Null,
            ),
            AnnouncementStoreError::AudienceNotCovered { source_id, topic } => typed(
                ANNOUNCEMENTS_REFUSED_CODE,
                "audience_not_covered",
                message,
                json!({ "source_id": source_id, "topic": topic }),
            ),
            AnnouncementStoreError::NoDirectRoute { source_id, topic } => typed(
                ANNOUNCEMENTS_REFUSED_CODE,
                "no_direct_route",
                message,
                json!({ "source_id": source_id, "topic": topic }),
            ),
            AnnouncementStoreError::RouteOverlap { source_id, member } => typed(
                ANNOUNCEMENTS_REFUSED_CODE,
                "route_overlap",
                message,
                json!({ "source_id": source_id, "member": member }),
            ),
            AnnouncementStoreError::ConversationMember { source_id } => typed(
                ANNOUNCEMENTS_REFUSED_CODE,
                "route_conflict",
                message,
                json!({ "conflict": "conversation", "source_id": source_id }),
            ),
            AnnouncementStoreError::DeclarationRollback { applied } => typed(
                ANNOUNCEMENTS_REFUSED_CODE,
                "declaration_rollback",
                message,
                json!({ "applied": applied }),
            ),
            AnnouncementStoreError::DeclarationDigestConflict { revision } => typed(
                ANNOUNCEMENTS_REFUSED_CODE,
                "declaration_digest_conflict",
                message,
                json!({ "revision": revision }),
            ),
            AnnouncementStoreError::RoleTemplateLimit { limit } => typed(
                ANNOUNCEMENTS_REFUSED_CODE,
                "role_template_limit",
                message,
                json!({ "limit": limit }),
            ),
            AnnouncementStoreError::SubscriptionIdTaken { subscription_id } => typed(
                ANNOUNCEMENTS_REFUSED_CODE,
                "subscription_id_taken",
                message,
                json!({ "subscription_id": subscription_id }),
            ),
            AnnouncementStoreError::NotHeld { batch } => typed(
                ANNOUNCEMENTS_REFUSED_CODE,
                "not_held",
                message,
                json!({ "batch_id": batch }),
            ),
            AnnouncementStoreError::IllegalTransition { .. } => JsonRpcError::new(-32000, message),
            AnnouncementStoreError::TooLarge(_) | AnnouncementStoreError::Invalid(_) => {
                invalid_params(message)
            }
            AnnouncementStoreError::Storage(_)
            | AnnouncementStoreError::RouteRequired { .. }
            | AnnouncementStoreError::RouteNotKept { .. } => JsonRpcError::new(-32000, message),
        },
    }
}

fn params<T: DeserializeOwned>(params: &Value) -> Result<T, JsonRpcError> {
    let value = if params.is_null() {
        Value::Object(serde_json::Map::new())
    } else {
        params.clone()
    };
    serde_json::from_value(value).map_err(invalid_params)
}

fn to_value(value: impl serde::Serialize) -> Result<Value, JsonRpcError> {
    serde_json::to_value(value).map_err(|error| JsonRpcError::new(-32000, error.to_string()))
}

/// The caller a request is answered for, as far as the members it may see
/// go. The host's own surface has no view: its caller sees every member.
///
/// A member the caller's view hides ([`AccessView::hides_agent`]: it is
/// private to another person) is that member's audience's alone, and so is
/// every record of it. The method runs with a name no member holds in the
/// place of such a member, and with an id nobody holds in the place of the
/// id of a subscription or a delivery of such a member, and the answer is
/// spelled back ([`Self::spelled_back`]): the method takes its own path for
/// a name or an id that does not exist, whatever state the owner is in.
/// Listings leave such a member's rows out, and answers about sources and
/// escalations leave such a member out.
///
/// A member is judged as the agent the view speaks of when this gateway's
/// runtime holds its mob, and every member is judged while that cannot be
/// said. The view knows the agents of the mobs held here by their names.
/// Only a member of a mob that is positively another gateway's (one the
/// contact directory files a peer under, and this gateway's runtime does
/// not hold) is another member under the same name, and is not judged. A
/// refusal that carries a member of a record, not of the request, is
/// answered without that member when the caller may not see it
/// ([`Self::refused`]).
struct Audience<'a> {
    view: Option<&'a AccessView>,
    /// The owner, which says whether a mob is a peer's.
    owner: Option<&'a AnnouncementService>,
    /// The mobs this gateway's runtime holds; `None` while that cannot be
    /// said.
    here: Option<BTreeSet<MobId>>,
    absent: ConsoleAbsentNames,
}

impl<'a> Audience<'a> {
    fn of(
        view: Option<&'a AccessView>,
        owner: Option<&'a AnnouncementService>,
        here: Option<BTreeSet<MobId>>,
    ) -> Self {
        Self {
            view,
            owner,
            here,
            absent: if view.is_some() {
                ConsoleAbsentNames::fresh()
            } else {
                ConsoleAbsentNames::default()
            },
        }
    }

    /// Whether the caller may not see `member`.
    fn hides(&self, member: &Member) -> bool {
        let Some(view) = self.view else {
            return false;
        };
        let another_gateways = self
            .here
            .as_ref()
            .is_some_and(|here| !here.contains(&member.mob_id))
            && self
                .owner
                .is_some_and(|owner| owner.files_peer(&member.mob_id));
        !another_gateways && view.hides_agent(member.identity.as_str())
    }

    /// Whether the caller may not see the member that made `announcement`:
    /// a member announces into its own mob.
    fn hides_announcer(&self, announcement: &Announcement) -> bool {
        match &announcement.announced_by {
            Announcer::Member { identity } => {
                self.hides(&Member::new(announcement.mob_id.clone(), identity.clone()))
            }
            Announcer::Host | Announcer::Source { .. } => false,
        }
    }

    /// The refusal as the caller receives it. A refusal that carries a
    /// member of a record, where the request named none (the subscriber a
    /// source's routes would reach twice, the member that announced under a
    /// taken key), is answered in its bare form when the caller may not see
    /// that member: the same kind, and neither the member nor the record.
    /// Every other refusal is the owner's own.
    fn refused(&self, error: AnnouncementError) -> JsonRpcError {
        match error {
            AnnouncementError::Store(AnnouncementStoreError::RouteOverlap {
                source_id,
                member,
            }) if self.hides(&member) => typed(
                ANNOUNCEMENTS_REFUSED_CODE,
                "route_overlap",
                route_overlap_without_member(&source_id),
                json!({ "source_id": source_id }),
            ),
            AnnouncementError::Store(AnnouncementStoreError::AnnouncementConflict { existing })
                if self.hides_announcer(&existing) =>
            {
                typed(
                    ANNOUNCEMENTS_CONFLICT_CODE,
                    "announcement_conflict",
                    AnnouncementStoreError::AnnouncementConflict { existing },
                    Value::Null,
                )
            }
            other => rpc_error(other),
        }
    }

    /// A name nobody holds that stands for `name`, within the bound of an
    /// id. A name too long to carry the mark is stood for by the mark alone.
    fn absent_name(&self, name: &str) -> String {
        let absent = self.absent.absent_spelling(name);
        if absent.len() <= MAX_ID_BYTES {
            absent
        } else {
            self.absent.absent_spelling("absent")
        }
    }

    /// `member` as the method is to take it: a member no mob holds in the
    /// place of one the caller may not see.
    fn member(&self, member: Member) -> Result<Member, JsonRpcError> {
        if !self.hides(&member) {
            return Ok(member);
        }
        let identity = MemberIdentity::new(self.absent_name(member.identity.as_str()))
            .map_err(|error| rpc_error(error.into()))?;
        Ok(Member::new(member.mob_id, identity))
    }

    /// The id a method on the subscription `id` is to take: an id nobody
    /// holds in the place of one whose subscription is of a member the
    /// caller may not see.
    async fn subscription_id(
        &self,
        service: &AnnouncementService,
        id: SubscriptionId,
    ) -> Result<SubscriptionId, JsonRpcError> {
        if self.view.is_none() {
            return Ok(id);
        }
        match service.subscription(&id).await.map_err(rpc_error)? {
            Some(found) if self.hides(&found.member) => {
                SubscriptionId::new(self.absent_name(id.as_str()))
                    .map_err(|error| rpc_error(error.into()))
            }
            _ => Ok(id),
        }
    }

    /// The id a method on the delivery `id` is to take: an id nobody holds
    /// in the place of one whose batch is of a member the caller may not
    /// see.
    async fn batch_id(
        &self,
        service: &AnnouncementService,
        id: BatchId,
    ) -> Result<BatchId, JsonRpcError> {
        if self.view.is_none() {
            return Ok(id);
        }
        match service.delivery(&id).await.map_err(rpc_error)? {
            Some(found) if self.hides(&found.member) => {
                BatchId::new(self.absent_name(id.as_str())).map_err(|error| rpc_error(error.into()))
            }
            _ => Ok(id),
        }
    }

    /// Whether an escalation about `subject` is one of a member the caller
    /// may not see: the member itself, a declared subscription of it, or a
    /// role template's child made for it.
    async fn hides_subject(
        &self,
        service: &AnnouncementService,
        subject: &EscalationSubject,
    ) -> Result<bool, JsonRpcError> {
        if self.view.is_none() {
            return Ok(false);
        }
        match subject {
            EscalationSubject::Member { member } => Ok(self.hides(member)),
            EscalationSubject::Declaration { record_kind, id } => match record_kind {
                DeclarationRecordKind::Source => Ok(false),
                DeclarationRecordKind::Subscription | DeclarationRecordKind::RoleChild => {
                    Ok(service
                        .declared_subscription_member(*record_kind, id)
                        .await
                        .map_err(rpc_error)?
                        .is_some_and(|member| self.hides(&member)))
                }
            },
            EscalationSubject::Source { .. } => Ok(false),
        }
    }

    /// `source` as the caller may see it: without the audience grants and
    /// the conversation members that name a member the caller may not see,
    /// and without a triage route of such a member.
    fn source(&self, mut source: AnnouncementSource) -> AnnouncementSource {
        if self.view.is_none() {
            return source;
        }
        let shown = |grant: &AudienceGrant| !matches!(grant, AudienceGrant::Member { member } if self.hides(member));
        source.audience.retain(&shown);
        source
            .conversation_members
            .retain(|member| !self.hides(member));
        source.routes.retain_mut(|route| match route {
            SourceRoute::Triage {
                member,
                forward_audience,
                ..
            } => {
                if let Some(grants) = forward_audience {
                    grants.retain(&shown);
                }
                !self.hides(member)
            }
            SourceRoute::Direct { .. } => true,
        });
        source
    }

    /// The answer as the caller receives it: every name and id the method
    /// took in the place of another spelled back as the caller gave it.
    fn spelled_back(&self, answer: Result<Value, JsonRpcError>) -> Result<Value, JsonRpcError> {
        if self.view.is_none() {
            return answer;
        }
        match answer {
            Ok(result) => Ok(self.absent.restore(result)),
            Err(mut error) => {
                if let Value::String(message) = self
                    .absent
                    .restore(Value::String(std::mem::take(&mut error.message)))
                {
                    error.message = message;
                }
                error.data = error.data.map(|data| self.absent.restore(data));
                Err(error)
            }
        }
    }
}

/// One page of a listing as its caller may see it: the rows of members the
/// caller may not see are left out and the listing is read on, so that
/// such a row neither shows nor takes a place on the page. `read` reads a
/// page after a cursor, `cursor` is a row's own, and `hidden` judges a row.
/// Without a view the page is the owner's own.
async fn seen_page<T, C, Fut>(
    audience: &Audience<'_>,
    limit: usize,
    after: Option<C>,
    cursor: impl Fn(&T) -> C,
    hidden: impl Fn(&T) -> bool,
    read: impl Fn(Option<C>, usize) -> Fut,
) -> Result<Vec<T>, JsonRpcError>
where
    Fut: Future<Output = Result<Vec<T>, AnnouncementError>>,
{
    if audience.view.is_none() {
        return read(after, limit).await.map_err(rpc_error);
    }
    let page = limit.clamp(1, MAX_PAGE);
    let mut after = after;
    let mut seen = Vec::new();
    loop {
        let rows = read(after.take(), limit).await.map_err(rpc_error)?;
        let ended = rows.len() < page;
        after = rows.last().map(&cursor);
        seen.extend(rows.into_iter().filter(|row| !hidden(row)));
        if ended || seen.len() >= page {
            break;
        }
    }
    seen.truncate(page);
    Ok(seen)
}

/// `meta/mob_announcements/announce`: the host announces into a mob hosted
/// here, this gateway's own by default.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AnnounceParams {
    #[serde(default)]
    mob_id: Option<MobId>,
    source_key: SourceKey,
    topics: BTreeSet<Topic>,
    body: String,
    #[serde(default)]
    correlation_id: Option<String>,
    #[serde(default)]
    causes: Vec<AnnouncementRef>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GetAnnouncementParams {
    mob_id: MobId,
    source_key: SourceKey,
}

fn default_start() -> StartFrontier {
    StartFrontier::Now
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PutSubscriptionParams {
    operation_id: OperationId,
    subscription_id: SubscriptionId,
    /// One member; exactly one of `member` and `members` is present.
    #[serde(default)]
    member: Option<Member>,
    /// Every roster member of a mob with a role: a role template. Host
    /// calls and declarations only; the member tool cannot make one.
    #[serde(default)]
    members: Option<RoleSelector>,
    topics: BTreeSet<Topic>,
    /// The mobs hosted here whose announcements the subscription takes;
    /// this gateway's own mob when absent.
    #[serde(default)]
    mobs: Option<BTreeSet<MobId>>,
    #[serde(default = "default_start")]
    start: StartFrontier,
    #[serde(default)]
    expected_revision: Option<u64>,
    /// How the member takes what the subscription captures.
    #[serde(default)]
    delivery: Option<SubscriptionDelivery>,
}

/// `meta/mob_announcements/declarations/apply`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ApplyDeclarationsParams {
    #[serde(default)]
    revision: Option<u64>,
    #[serde(default)]
    declarations: Option<Declarations>,
}

/// A method that takes no params.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EmptyParams {}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ChangeSubscriptionParams {
    operation_id: OperationId,
    subscription_id: SubscriptionId,
    expected_revision: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SubscriptionParams {
    subscription_id: SubscriptionId,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ListSubscriptionsParams {
    #[serde(default)]
    member: Option<Member>,
    #[serde(default)]
    after: Option<SubscriptionId>,
    #[serde(default)]
    limit: Option<usize>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ListDeliveriesParams {
    #[serde(default)]
    member: Option<Member>,
    #[serde(default)]
    subscription_id: Option<SubscriptionId>,
    #[serde(default)]
    after: Option<BatchId>,
    #[serde(default)]
    limit: Option<usize>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DeliveryParams {
    batch_id: BatchId,
}

/// `meta/mob_announcements/deliveries/redeliver` and `.../dismiss`: an
/// operator's remedy for what a batch holds after its turn ended
/// unhandled.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HeldDeliveryParams {
    operation_id: OperationId,
    batch_id: BatchId,
}

/// `meta/mob_announcements/sources/put`: declare a source, replace its
/// declaration at its revision, or start its next generation.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PutSourceParams {
    operation_id: OperationId,
    #[serde(default)]
    mob_id: Option<MobId>,
    source_id: SourceId,
    #[serde(default)]
    expected_revision: Option<u64>,
    #[serde(default)]
    new_generation: bool,
    identity: SourceIdentity,
    #[serde(default)]
    connection: Option<SourceConnection>,
    observation: ObservationCapability,
    topics: BTreeSet<Topic>,
    audience: Vec<AudienceGrant>,
    #[serde(default)]
    routes: Option<Vec<SourceRoute>>,
    #[serde(default)]
    conversation_members: Vec<Member>,
    #[serde(default)]
    start: Option<SourceStart>,
    #[serde(default)]
    delivery: Option<SourceDelivery>,
    #[serde(default)]
    health_policy: Option<SourceHealthPolicy>,
    #[serde(default)]
    runner: Option<SourceRunner>,
}

/// `meta/mob_announcements/sources/{pause,resume,end}`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ChangeSourceParams {
    operation_id: OperationId,
    #[serde(default)]
    mob_id: Option<MobId>,
    source_id: SourceId,
    expected_revision: u64,
    /// `resume` only: resume from a fresh baseline instead of the kept
    /// cursor.
    #[serde(default)]
    rebaseline: Option<bool>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceParams {
    #[serde(default)]
    mob_id: Option<MobId>,
    source_id: SourceId,
    /// Also list the streams the current generation onboarded.
    #[serde(default)]
    include_streams: bool,
}

/// `meta/mob_announcements/sources/advance`: one collector step.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AdvanceSourceParams {
    operation_id: OperationId,
    #[serde(default)]
    mob_id: Option<MobId>,
    source_id: SourceId,
    expected_revision: u64,
    expected_version: u64,
    principal: Principal,
    next_cursor: Option<String>,
    #[serde(default)]
    page: Option<SourcePage>,
    #[serde(default)]
    coverage: Option<Coverage>,
    #[serde(default)]
    items: Vec<SourceItem>,
    #[serde(default)]
    streams: Vec<StreamOnboarding>,
    #[serde(default)]
    rebaseline: Option<Rebaseline>,
    #[serde(default)]
    observed: Option<Observed>,
    #[serde(default)]
    collected_at_ms: Option<u64>,
}

/// `meta/mob_announcements/audit/list`: the counted refusals and
/// withholdings of one mob's sources, by source, by member, by action, or
/// any of them.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AuditListParams {
    /// The mob whose sources' audit is read: this gateway's own when absent.
    #[serde(default)]
    mob_id: Option<MobId>,
    #[serde(default)]
    source_id: Option<SourceId>,
    #[serde(default)]
    member: Option<Member>,
    #[serde(default)]
    action: Option<AuditAction>,
    /// Start after this entry id.
    #[serde(default)]
    after: Option<u64>,
    #[serde(default)]
    limit: Option<usize>,
}

/// `meta/mob_announcements/sources/record_failure`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RecordFailureParams {
    operation_id: OperationId,
    #[serde(default)]
    mob_id: Option<MobId>,
    source_id: SourceId,
    expected_revision: u64,
    expected_version: u64,
    principal: Principal,
    failure: SourceFailure,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ListSourcesParams {
    /// The mob whose sources are listed: this gateway's own when absent.
    #[serde(default)]
    mob_id: Option<MobId>,
    /// Start after this source of the listed mob.
    #[serde(default)]
    after: Option<SourceId>,
    #[serde(default)]
    state: Option<SourceState>,
    #[serde(default)]
    limit: Option<usize>,
    /// Only sources with an open escalation.
    #[serde(default)]
    escalated: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MemberGetParams {
    member: Member,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MembersListParams {
    /// Start after this member, in member key order.
    #[serde(default)]
    after: Option<Member>,
    #[serde(default)]
    limit: Option<usize>,
    /// Only members with an open escalation.
    #[serde(default)]
    escalated: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EscalationsListParams {
    /// Read the events after this cursor; 0, the default, reads from the
    /// first one kept.
    #[serde(default)]
    after: u64,
    #[serde(default)]
    limit: Option<usize>,
    /// Answer the open escalations instead of events.
    #[serde(default)]
    open: bool,
}

/// A source as a list shows it: its principal as its kind and digest only,
/// and neither its cursor token nor a failure's detail, which only
/// `sources/get` returns.
fn listed_source(source: &AnnouncementSource) -> Result<Value, JsonRpcError> {
    let mut value = to_value(source)?;
    if let Some(cursor) = value.get_mut("cursor").and_then(Value::as_object_mut) {
        cursor.remove("token");
    }
    if let Some(failure) = value
        .pointer_mut("/health/last_failure")
        .and_then(Value::as_object_mut)
    {
        failure.remove("detail");
    }
    let principal = &source.identity.principal;
    let digest = principal
        .digest()
        .map_err(|error| rpc_error(error.into()))?;
    if let Some(listed) = value.pointer_mut("/identity/principal") {
        *listed = json!({ "kind": principal.kind(), "digest": digest });
    }
    Ok(value)
}

const DEFAULT_LIMIT: usize = 100;

/// The result of an announce call. It counts the subscriptions that
/// captured the announcement and never names them: who subscribes is not
/// the announcer's to learn.
#[derive(serde::Serialize)]
struct AnnounceResult {
    announcement: super::types::Announcement,
    replayed: bool,
    captured: usize,
}

/// Answer one mob announcement method. `service` is `None` when the host did
/// not enable mob announcements; `own_mob` is this host's mob, the default
/// for a call that names none.
pub async fn handle_announcement_method(
    service: Option<&AnnouncementService>,
    own_mob: &MobId,
    method: &str,
    raw: &Value,
) -> Result<Value, JsonRpcError> {
    handle_announcement_method_seen_by(service, own_mob, None, method, raw).await
}

/// [`handle_announcement_method`] for a caller who is not the host: `view`
/// is the caller's view of the mob's members, and the method answers for
/// the members that view shows ([`Audience`]). Without a view the caller is
/// the host, which sees every member.
pub(crate) async fn handle_announcement_method_seen_by(
    service: Option<&AnnouncementService>,
    own_mob: &MobId,
    view: Option<&AccessView>,
    method: &str,
    raw: &Value,
) -> Result<Value, JsonRpcError> {
    // The view speaks of the agents of the mobs this gateway's runtime
    // holds: this host's own mob among them.
    let here = match (view, service) {
        (Some(_), Some(service)) => service.mobs_held_here().await.map(|mut here| {
            here.insert(own_mob.clone());
            here
        }),
        _ => None,
    };
    let audience = Audience::of(view, service, here);
    let answer = answer_announcement_method(service, own_mob, &audience, method, raw).await;
    audience.spelled_back(answer)
}

/// The text of a route overlap refusal that names no member: what a caller
/// who may not see the subscriber is answered, and what a declared source's
/// pending refusal shows such a caller.
pub(super) fn route_overlap_without_member(source_id: &impl std::fmt::Display) -> String {
    format!(
        "a subscriber would be reachable both directly and through a forward of source \
         {source_id}"
    )
}

async fn answer_announcement_method(
    service: Option<&AnnouncementService>,
    own_mob: &MobId,
    audience: &Audience<'_>,
    method: &str,
    raw: &Value,
) -> Result<Value, JsonRpcError> {
    if !is_announcement_method(method) {
        return Err(JsonRpcError::new(-32601, "Method not found"));
    }
    let Some(service) = service else {
        return Err(announcements_unavailable_error());
    };
    let hidden = |member: &Member| audience.hides(member);
    // The judge the owner is handed where it keeps a hidden member's
    // records as they are: none for the host.
    let judge: Option<HiddenMember<'_>> = if audience.view.is_some() {
        Some(&hidden)
    } else {
        None
    };
    // Every refusal of the owner goes to the caller through the audience.
    let refused = |error: AnnouncementError| audience.refused(error);
    match method {
        "meta/mob_announcements/announce" => {
            let request: AnnounceParams = params(raw)?;
            let outcome = service
                .announce(AnnounceRequest {
                    mob_id: request.mob_id.unwrap_or_else(|| own_mob.clone()),
                    announced_by: Announcer::Host,
                    source_key: request.source_key,
                    topics: request.topics,
                    body: request.body,
                    correlation_id: request.correlation_id,
                    causes: request.causes,
                })
                .await
                .map_err(refused)?;
            to_value(AnnounceResult {
                captured: outcome.captured_for.len(),
                announcement: outcome.announcement,
                replayed: outcome.replayed,
            })
        }
        "meta/mob_announcements/get" => {
            let request: GetAnnouncementParams = params(raw)?;
            // An announcement a member the caller may not see made is
            // answered as a key nothing was announced under.
            let announcement = service
                .announcement(&request.mob_id, &request.source_key)
                .await
                .map_err(refused)?
                .filter(|announcement| !audience.hides_announcer(announcement));
            Ok(json!({ "announcement": to_value(announcement)? }))
        }
        "meta/mob_announcements/subscriptions/put" => {
            let request: PutSubscriptionParams = params(raw)?;
            let mobs = request
                .mobs
                .unwrap_or_else(|| BTreeSet::from([own_mob.clone()]));
            let member = match (request.member, request.members) {
                (Some(member), None) => audience.member(member)?,
                (None, Some(members)) => {
                    // A template starts now: it never backfills.
                    if request.start != StartFrontier::Now {
                        return Err(invalid_params(
                            "a role-selected subscription starts now".to_string(),
                        ));
                    }
                    let template = service
                        .put_role_template(
                            &request.operation_id,
                            PutRoleTemplate {
                                id: request.subscription_id,
                                members,
                                topics: request.topics,
                                mobs,
                                expected_revision: request.expected_revision,
                                delivery: request.delivery.unwrap_or_default(),
                            },
                        )
                        .await
                        .map_err(refused)?;
                    return Ok(json!({ "subscription": to_value(template)? }));
                }
                _ => {
                    return Err(invalid_params(
                        "a subscription names exactly one of member and members".to_string(),
                    ));
                }
            };
            let subscription = service
                .put_subscription(
                    &request.operation_id,
                    PutSubscription {
                        id: request.subscription_id,
                        member,
                        selector: Selector {
                            topics: request.topics,
                            mobs,
                        },
                        start: request.start,
                        expected_revision: request.expected_revision,
                        route: None,
                        member_mob_cursor: None,
                        child_limits: ChildMemberLimits::default(),
                        delivery: request.delivery.unwrap_or_default(),
                    },
                )
                .await
                .map_err(refused)?;
            Ok(json!({ "subscription": to_value(subscription)? }))
        }
        "meta/mob_announcements/subscriptions/pause"
        | "meta/mob_announcements/subscriptions/resume"
        | "meta/mob_announcements/subscriptions/cancel" => {
            let request: ChangeSubscriptionParams = params(raw)?;
            let change = match method {
                "meta/mob_announcements/subscriptions/pause" => SubscriptionChange::Pause,
                "meta/mob_announcements/subscriptions/resume" => SubscriptionChange::Resume,
                _ => SubscriptionChange::Cancel,
            };
            let subscription_id = audience
                .subscription_id(service, request.subscription_id)
                .await?;
            let mut outcome = service
                .change_subscription_or_template(
                    &request.operation_id,
                    &subscription_id,
                    request.expected_revision,
                    change,
                )
                .await
                .map_err(refused)?;
            // A role template's change lists its children: none of a member
            // the caller may not see.
            if let SubscriptionVerbOutcome::Template(template) = &mut outcome {
                template
                    .children
                    .retain(|child| !audience.hides(&child.member));
            }
            to_value(outcome)
        }
        "meta/mob_announcements/subscriptions/get" => {
            let request: SubscriptionParams = params(raw)?;
            let subscription_id = audience
                .subscription_id(service, request.subscription_id)
                .await?;
            let subscription = service
                .subscription(&subscription_id)
                .await
                .map_err(refused)?;
            // Why the member's deliveries wait now, if they do: a
            // projection of the owner's latest attempt, never stored.
            let marker = subscription
                .as_ref()
                .and_then(|subscription| service.suspend_marker(&subscription.member));
            let mut found = json!({
                "subscription": to_value(&subscription)?,
                "marker": to_value(marker)?,
            });
            // A role template's id reads the template beside no subscription.
            if subscription.is_none()
                && let Some(template) = service
                    .role_template(&subscription_id)
                    .await
                    .map_err(refused)?
            {
                found["template"] = to_value(template)?;
            }
            Ok(found)
        }
        "meta/mob_announcements/subscriptions/list" => {
            let request: ListSubscriptionsParams = params(raw)?;
            let member = request
                .member
                .map(|member| audience.member(member))
                .transpose()?;
            let subscriptions = seen_page(
                audience,
                request.limit.unwrap_or(DEFAULT_LIMIT),
                request.after,
                |row: &Subscription| row.id.clone(),
                |row: &Subscription| audience.hides(&row.member),
                |after: Option<SubscriptionId>, limit| {
                    let member = member.as_ref();
                    async move { service.subscriptions(member, after.as_ref(), limit).await }
                },
            )
            .await?;
            Ok(json!({ "subscriptions": to_value(subscriptions)? }))
        }
        "meta/mob_announcements/deliveries/list" => {
            let request: ListDeliveriesParams = params(raw)?;
            let member = request
                .member
                .map(|member| audience.member(member))
                .transpose()?;
            let subscription = request.subscription_id;
            let batches = seen_page(
                audience,
                request.limit.unwrap_or(DEFAULT_LIMIT),
                request.after,
                |row: &(Batch, Option<BatchHandling>)| row.0.id.clone(),
                |row: &(Batch, Option<BatchHandling>)| audience.hides(&row.0.member),
                |after: Option<BatchId>, limit| {
                    let query = BatchQuery {
                        member: member.clone(),
                        subscription: subscription.clone(),
                        after,
                        limit,
                    };
                    async move { service.deliveries_with_handling(&query).await }
                },
            )
            .await?;
            // Each batch with what became of its turn: null until admitted.
            let mut listed = Vec::with_capacity(batches.len());
            for (batch, handling) in batches {
                let mut entry = to_value(batch)?;
                if let Value::Object(fields) = &mut entry {
                    fields.insert("handling".to_string(), to_value(handling)?);
                }
                listed.push(entry);
            }
            Ok(json!({ "deliveries": listed }))
        }
        "meta/mob_announcements/deliveries/observe" => {
            let request: DeliveryParams = params(raw)?;
            let batch_id = audience.batch_id(service, request.batch_id).await?;
            let observation = service.observe_delivery(&batch_id).await.map_err(refused)?;
            to_value(observation)
        }
        "meta/mob_announcements/deliveries/reconcile" => {
            let request: DeliveryParams = params(raw)?;
            let batch_id = audience.batch_id(service, request.batch_id).await?;
            let observation = service
                .reconcile_delivery(&batch_id)
                .await
                .map_err(refused)?;
            to_value(observation)
        }
        "meta/mob_announcements/deliveries/redeliver" => {
            let request: HeldDeliveryParams = params(raw)?;
            let batch_id = audience.batch_id(service, request.batch_id).await?;
            let handling = service
                .redeliver_held(&request.operation_id, &batch_id)
                .await
                .map_err(refused)?;
            Ok(json!({ "handling": to_value(handling)? }))
        }
        "meta/mob_announcements/deliveries/dismiss" => {
            let request: HeldDeliveryParams = params(raw)?;
            let batch_id = audience.batch_id(service, request.batch_id).await?;
            let handling = service
                .dismiss_held(&request.operation_id, &batch_id)
                .await
                .map_err(refused)?;
            Ok(json!({ "handling": to_value(handling)? }))
        }
        "meta/mob_announcements/sources/put" => {
            let request: PutSourceParams = params(raw)?;
            let config = SourceConfig {
                identity: request.identity,
                connection: request.connection,
                observation: request.observation,
                topics: request.topics,
                audience: request.audience,
                routes: request.routes.unwrap_or_else(|| {
                    vec![SourceRoute::Direct {
                        topics: None,
                        streams: None,
                    }]
                }),
                conversation_members: request.conversation_members,
                start: request.start.unwrap_or_default(),
                delivery: request.delivery.unwrap_or_default(),
                health_policy: request.health_policy.unwrap_or_default(),
                runner: request.runner,
            };
            // A caller who is not the host names only seated members it may
            // see: any other name is one of an unknown member, whoever
            // holds it.
            if let Some(judge) = judge {
                service
                    .admit_source_members(&config, judge)
                    .await
                    .map_err(refused)?;
            }
            let source = service
                .put_source(
                    &request.operation_id,
                    PutSource {
                        mob_id: request.mob_id.unwrap_or_else(|| own_mob.clone()),
                        source_id: request.source_id,
                        expected_revision: request.expected_revision,
                        new_generation: request.new_generation,
                        config,
                    },
                )
                .await
                .map_err(refused)?;
            let affected_subscriptions = service
                .source_bound_subscriptions(&source.mob_id, &source.source_id)
                .await
                .map_err(refused)?;
            let source = audience.source(source);
            let value = to_value(&source)?;
            Ok(json!({
                "source": with_health(service, &source, value).await?,
                "affected_subscriptions": affected_subscriptions,
                "delivery_readiness": service.local_delivery(),
            }))
        }
        "meta/mob_announcements/sources/pause"
        | "meta/mob_announcements/sources/resume"
        | "meta/mob_announcements/sources/end" => {
            let request: ChangeSourceParams = params(raw)?;
            let change = match (method, request.rebaseline) {
                ("meta/mob_announcements/sources/resume", rebaseline) => SourceChange::Resume {
                    rebaseline: rebaseline.unwrap_or(false),
                },
                (_, Some(_)) => {
                    return Err(invalid_params("only sources/resume takes rebaseline"));
                }
                ("meta/mob_announcements/sources/pause", None) => SourceChange::Pause,
                (_, None) => SourceChange::End,
            };
            let source = service
                .change_source(
                    &request.operation_id,
                    &request.mob_id.unwrap_or_else(|| own_mob.clone()),
                    &request.source_id,
                    request.expected_revision,
                    change,
                )
                .await
                .map_err(refused)?;
            Ok(json!({ "source": to_value(audience.source(source))? }))
        }
        "meta/mob_announcements/sources/get" => {
            let request: SourceParams = params(raw)?;
            let mob_id = request.mob_id.unwrap_or_else(|| own_mob.clone());
            let source = service
                .source(&mob_id, &request.source_id)
                .await
                .map_err(refused)?
                .map(|source| audience.source(source));
            let value = to_value(&source)?;
            let value = match &source {
                Some(found) => with_health(service, found, value).await?,
                None => value,
            };
            let mut answer = json!({ "source": value });
            if request.include_streams {
                let streams = service
                    .source_streams(&mob_id, &request.source_id)
                    .await
                    .map_err(refused)?;
                answer["streams"] = to_value(streams)?;
            }
            Ok(answer)
        }
        "meta/mob_announcements/sources/advance" => {
            let request: AdvanceSourceParams = params(raw)?;
            let mut advance = service
                .advance_source(
                    &request.operation_id,
                    AdvanceSource {
                        mob_id: request.mob_id.unwrap_or_else(|| own_mob.clone()),
                        source_id: request.source_id,
                        expected_revision: request.expected_revision,
                        expected_version: request.expected_version,
                        principal: request.principal,
                        next_cursor: request.next_cursor,
                        page: request.page,
                        coverage: request.coverage,
                        items: request.items,
                        streams: request.streams,
                        rebaseline: request.rebaseline,
                        observed: request.observed,
                        collected_at_ms: request.collected_at_ms,
                    },
                )
                .await
                .map_err(refused)?;
            advance.source = audience.source(advance.source);
            to_value(advance)
        }
        "meta/mob_announcements/audit/list" => {
            let request: AuditListParams = params(raw)?;
            let mob_id = request.mob_id.unwrap_or_else(|| own_mob.clone());
            let member = request
                .member
                .map(|member| audience.member(member))
                .transpose()?;
            let (source_id, action) = (request.source_id, request.action);
            let rows = seen_page(
                audience,
                request.limit.unwrap_or(DEFAULT_LIMIT),
                request.after,
                |row: &AuditRow| row.id,
                |row: &AuditRow| audience.hides(&row.member),
                |after: Option<u64>, limit| {
                    let query = AuditQuery {
                        mob_id: Some(mob_id.clone()),
                        source_id: source_id.clone(),
                        member: member.clone(),
                        action,
                        after,
                        limit,
                    };
                    async move { service.audit(&query).await }
                },
            )
            .await?;
            Ok(json!({ "entries": to_value(rows)? }))
        }
        "meta/mob_announcements/sources/record_failure" => {
            let request: RecordFailureParams = params(raw)?;
            let source = service
                .record_source_failure(
                    &request.operation_id,
                    RecordSourceFailure {
                        mob_id: request.mob_id.unwrap_or_else(|| own_mob.clone()),
                        source_id: request.source_id,
                        expected_revision: request.expected_revision,
                        expected_version: request.expected_version,
                        principal: request.principal,
                        failure: request.failure,
                    },
                )
                .await
                .map_err(refused)?;
            Ok(json!({ "source": to_value(audience.source(source))? }))
        }
        "meta/mob_announcements/sources/list" => {
            let request: ListSourcesParams = params(raw)?;
            let mob_id = request.mob_id.unwrap_or_else(|| own_mob.clone());
            let limit = request
                .limit
                .unwrap_or(DEFAULT_LIMIT)
                .clamp(1, super::types::MAX_PAGE);
            let mut after = request.after.map(|source_id| (mob_id.clone(), source_id));
            let mut listed = Vec::new();
            // Escalated sources are picked here, after the store's page: the
            // read goes on until the page is full or the sources end, so an
            // escalated source behind a page of others is still listed.
            loop {
                let sources = service
                    .sources(&SourceQuery {
                        after: after.clone(),
                        mob_id: Some(mob_id.clone()),
                        state: request.state,
                        limit,
                    })
                    .await
                    .map_err(refused)?;
                let ended = sources.len() < limit;
                after = sources
                    .last()
                    .map(|source| (source.mob_id.clone(), source.source_id.clone()));
                for source in sources {
                    let source = audience.source(source);
                    let value = with_health(service, &source, listed_source(&source)?).await?;
                    let escalated = value
                        .pointer("/health/escalations")
                        .and_then(Value::as_array)
                        .is_some_and(|escalations| !escalations.is_empty());
                    if (!request.escalated || escalated) && listed.len() < limit {
                        listed.push(value);
                    }
                }
                if ended || !request.escalated || listed.len() >= limit {
                    break;
                }
            }
            Ok(json!({ "sources": listed }))
        }
        "meta/mob_announcements/members/get" => {
            let request: MemberGetParams = params(raw)?;
            let member = audience.member(request.member)?;
            let read = service.member_read(&member).await.map_err(refused)?;
            to_value(read)
        }
        "meta/mob_announcements/members/list" => {
            let request: MembersListParams = params(raw)?;
            let escalated = request.escalated;
            let reads = seen_page(
                audience,
                request
                    .limit
                    .unwrap_or(DEFAULT_LIMIT)
                    .clamp(1, super::types::MAX_PAGE),
                request.after,
                |row: &MemberRead| row.member.clone(),
                |row: &MemberRead| audience.hides(&row.member),
                |after: Option<Member>, limit| async move {
                    service.member_reads(after.as_ref(), limit, escalated).await
                },
            )
            .await?;
            Ok(json!({ "members": to_value(reads)? }))
        }
        "meta/mob_announcements/escalations/list" => {
            let request: EscalationsListParams = params(raw)?;
            let limit = request.limit.unwrap_or(DEFAULT_LIMIT);
            if request.open {
                let next = service.last_escalation_cursor().await.map_err(refused)?;
                let mut open = Vec::new();
                for escalation in service.open_escalations(None).await.map_err(refused)? {
                    if !audience.hides_subject(service, &escalation.subject).await? {
                        open.push(escalation);
                    }
                }
                Ok(json!({ "escalations": to_value(open)?, "next": next }))
            } else if audience.view.is_none() {
                let (events, next) = service
                    .escalation_events(request.after, limit)
                    .await
                    .map_err(refused)?;
                Ok(json!({ "events": to_value(events)?, "next": next }))
            } else {
                // The events the caller may see, read on past the others
                // until the page is full or the events end; `next` is where
                // the caller reads on from.
                let page = limit.clamp(1, MAX_PAGE);
                let mut next = request.after;
                let mut seen = Vec::new();
                'pages: loop {
                    let (events, read_to) = service
                        .escalation_events(next, limit)
                        .await
                        .map_err(refused)?;
                    let ended = events.len() < page;
                    for event in events {
                        if seen.len() >= page {
                            break 'pages;
                        }
                        next = event.cursor;
                        if !audience.hides_subject(service, &event.subject).await? {
                            seen.push(event);
                        }
                    }
                    next = read_to;
                    if ended || seen.len() >= page {
                        break;
                    }
                }
                Ok(json!({ "events": to_value(seen)?, "next": next }))
            }
        }
        "meta/mob_announcements/declarations/apply" => {
            let request: ApplyDeclarationsParams = params(raw)?;
            // Both present (inline declarations), or both absent: the
            // gateway's sidecar is read again.
            let declarations = match (request.revision, request.declarations) {
                (Some(revision), Some(declarations)) => {
                    if declarations.revision != revision {
                        return Err(invalid_params(format!(
                            "the declarations name revision {}, the apply {revision}",
                            declarations.revision
                        )));
                    }
                    Some(declarations)
                }
                (None, None) => None,
                _ => {
                    return Err(invalid_params(
                        "an apply carries both revision and declarations, or neither".to_string(),
                    ));
                }
            };
            let view = service
                .apply_declarations_seen_by(declarations, judge)
                .await
                .map_err(refused)?;
            to_value(view)
        }
        "meta/mob_announcements/declarations/get" => {
            let _: EmptyParams = params(raw)?;
            to_value(service.declarations_seen_by(judge).await.map_err(refused)?)
        }
        _ => Err(JsonRpcError::new(-32601, "Method not found")),
    }
}
