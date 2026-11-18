#![allow(clippy::expect_used, clippy::panic)]

//! The notices a WorkGraph service gives its commit observer: one for each
//! item or attention binding whose revision a committed write advanced, and
//! none for a refused write, a read, or a write that advances neither.

use std::sync::{Arc, Mutex};

use meerkat_core::SessionId;
use meerkat_workgraph::{
    AddEvidenceRequest, AttentionBindingRequest, AttentionListRequest, AttentionPauseRequest,
    AttentionProjectionRequest, AttentionReassignRequest, AttentionResumeRequest,
    ClaimWorkItemRequest, CloseWorkItemRequest, CreateWorkItemRequest, GoalAttentionTarget,
    GoalCreateRequest, GoalCreateResult, LinkWorkItemsRequest, MemoryWorkGraphStore,
    ReleaseWorkItemRequest, UpdateWorkItemRequest, WorkAdmissionKey, WorkEdgeKind,
    WorkGraphCommitNotice, WorkGraphCommitObserver, WorkGraphCommitOrigin, WorkGraphService,
    WorkItem, WorkOwner, WorkOwnerKey, WorkStatus,
};
use serde_json::json;

const BOUND_SESSION: &str = "019e63c2-0000-7000-8000-0000000000c1";
const OTHER_SESSION: &str = "019e63c2-0000-7000-8000-0000000000c2";

/// Keeps every notice it is given, in order.
#[derive(Default)]
struct Recorder {
    notices: Mutex<Vec<WorkGraphCommitNotice>>,
}

impl Recorder {
    /// The notices given since the last call, in order.
    fn taken(&self) -> Vec<WorkGraphCommitNotice> {
        std::mem::take(&mut *self.notices.lock().expect("recorded notices"))
    }
}

impl WorkGraphCommitObserver for Recorder {
    fn committed(&self, notice: WorkGraphCommitNotice) {
        self.notices.lock().expect("recorded notices").push(notice);
    }
}

/// A service over a memory store with a recording observer.
fn observed_service() -> (WorkGraphService, Arc<Recorder>) {
    let recorder = Arc::new(Recorder::default());
    let service = WorkGraphService::new(Arc::new(MemoryWorkGraphStore::new()))
        .with_commit_observer(recorder.clone());
    (service, recorder)
}

/// The notice of a write that advanced `item`'s revision.
fn of_item(item: &WorkItem, origin: WorkGraphCommitOrigin) -> WorkGraphCommitNotice {
    WorkGraphCommitNotice {
        realm_id: item.realm_id.clone(),
        namespace: item.namespace.clone(),
        item_id: item.id.clone(),
        binding_id: None,
        origin,
    }
}

/// The notice of a write that advanced the revision of `goal`'s binding.
fn of_binding(goal: &GoalCreateResult, origin: WorkGraphCommitOrigin) -> WorkGraphCommitNotice {
    WorkGraphCommitNotice {
        realm_id: goal.attention.work_ref.realm_id.clone(),
        namespace: goal.attention.work_ref.namespace.clone(),
        item_id: goal.attention.work_ref.item_id.clone(),
        binding_id: Some(goal.attention.binding_id.clone()),
        origin,
    }
}

async fn created(service: &WorkGraphService, title: &str) -> WorkItem {
    service
        .create(CreateWorkItemRequest {
            title: title.to_string(),
            ..Default::default()
        })
        .await
        .expect("create the item")
}

/// A goal whose attention is bound to `session`, in `mode`.
async fn goal_for(service: &WorkGraphService, session: &str, mode: &str) -> GoalCreateResult {
    let request: GoalCreateRequest = serde_json::from_value(json!({
        "title": "a goal",
        "target": { "kind": "session", "session_id": session },
        "mode": mode,
    }))
    .expect("a goal request");
    service.create_goal(request).await.expect("create the goal")
}

/// A write of an item is told once, for the item, attributed to nobody when
/// the service was given no origin: its creation, an update, a claim, a
/// release and added evidence each give one notice.
#[tokio::test]
async fn each_item_write_is_told_once_for_the_item() {
    let (service, recorder) = observed_service();
    let unattributed = WorkGraphCommitOrigin::Unattributed;

    let item = created(&service, "an item").await;
    assert_eq!(recorder.taken(), vec![of_item(&item, unattributed.clone())]);

    let update: UpdateWorkItemRequest = serde_json::from_value(json!({
        "id": item.id,
        "expected_revision": item.revision,
        "description": "revised",
    }))
    .expect("an update");
    let updated = service.update(update).await.expect("update the item");
    assert_eq!(
        recorder.taken(),
        vec![of_item(&updated, unattributed.clone())]
    );

    let claimed = service
        .claim(ClaimWorkItemRequest {
            id: updated.id.clone(),
            realm_id: None,
            namespace: None,
            expected_revision: updated.revision,
            owner: WorkOwner::new(WorkOwnerKey::agent("worker-1").expect("owner key")),
            lease_seconds: None,
            lease_expires_at: None,
        })
        .await
        .expect("claim the item");
    assert_eq!(
        recorder.taken(),
        vec![of_item(&claimed, unattributed.clone())]
    );

    let evidenced = service
        .add_evidence(AddEvidenceRequest {
            id: claimed.id.clone(),
            realm_id: None,
            namespace: None,
            expected_revision: claimed.revision,
            evidence: serde_json::from_value(json!({
                "kind": "note",
                "id": "n-1",
                "summary": "seen",
            }))
            .expect("an evidence reference"),
        })
        .await
        .expect("add evidence");
    assert_eq!(
        recorder.taken(),
        vec![of_item(&evidenced, unattributed.clone())]
    );

    let released = service
        .release(ReleaseWorkItemRequest {
            id: evidenced.id.clone(),
            realm_id: None,
            namespace: None,
            expected_revision: evidenced.revision,
        })
        .await
        .expect("release the item");
    assert_eq!(recorder.taken(), vec![of_item(&released, unattributed)]);
}

/// A goal's creation advances its item and its binding and is told once
/// for each. A pause and a resume of the binding are told for the binding.
/// The close of the item is told for the item alone: the stop of its
/// binding in the same write gets no notice of its own.
#[tokio::test]
async fn a_goal_is_told_for_its_item_and_its_binding() {
    let (service, recorder) = observed_service();
    let unattributed = WorkGraphCommitOrigin::Unattributed;

    let goal = goal_for(&service, BOUND_SESSION, "pursue").await;
    assert_eq!(
        recorder.taken(),
        vec![
            of_item(&goal.item, unattributed.clone()),
            of_binding(&goal, unattributed.clone()),
        ]
    );

    let paused = service
        .pause_attention(AttentionPauseRequest {
            binding_id: goal.attention.binding_id.clone(),
            realm_id: None,
            namespace: None,
            expected_revision: goal.attention.machine_state.revision,
            until: None,
        })
        .await
        .expect("pause the binding")
        .attention;
    assert_eq!(
        recorder.taken(),
        vec![of_binding(&goal, unattributed.clone())]
    );

    service
        .resume_attention(AttentionResumeRequest {
            binding_id: goal.attention.binding_id.clone(),
            realm_id: None,
            namespace: None,
            expected_revision: paused.machine_state.revision,
        })
        .await
        .expect("resume the binding");
    assert_eq!(
        recorder.taken(),
        vec![of_binding(&goal, unattributed.clone())]
    );

    let closed = service
        .close(CloseWorkItemRequest {
            id: goal.item.id.clone(),
            realm_id: None,
            namespace: None,
            expected_revision: goal.item.revision,
            status: WorkStatus::Cancelled,
        })
        .await
        .expect("close the item");
    assert_eq!(recorder.taken(), vec![of_item(&closed, unattributed)]);
}

/// A reassign supersedes one binding and makes another: it is told once
/// for each of the two.
#[tokio::test]
async fn a_reassign_is_told_for_both_bindings() {
    let (service, recorder) = observed_service();
    let goal = goal_for(&service, BOUND_SESSION, "coordinate").await;
    let projection = service
        .attention_projection(AttentionProjectionRequest {
            binding_id: goal.attention.binding_id.clone(),
            realm_id: None,
            namespace: None,
        })
        .await
        .expect("the binding's projection")
        .projection;
    recorder.taken();

    let reassigned = service
        .reassign_attention(AttentionReassignRequest {
            binding_id: goal.attention.binding_id.clone(),
            realm_id: None,
            namespace: None,
            expected_revision: goal.attention.machine_state.revision,
            authority_projection: projection,
            target: GoalAttentionTarget::Session {
                session_id: SessionId::parse(OTHER_SESSION).expect("session id"),
            },
        })
        .await
        .expect("reassign the binding");
    assert_eq!(reassigned.previous.binding_id, goal.attention.binding_id);
    assert_ne!(reassigned.attention.binding_id, goal.attention.binding_id);
    let mut told = recorder
        .taken()
        .into_iter()
        .map(|notice| {
            assert_eq!(
                notice.item_id, goal.item.id,
                "both are bindings of the item"
            );
            assert_eq!(notice.origin, WorkGraphCommitOrigin::Unattributed);
            notice.binding_id.expect("a binding's notice")
        })
        .collect::<Vec<_>>();
    told.sort();
    let mut expected = vec![
        reassigned.previous.binding_id,
        reassigned.attention.binding_id,
    ];
    expected.sort();
    assert_eq!(told, expected);
}

/// Nothing is told for a refused write, for a read, or for a write that
/// advances neither an item's nor a binding's revision: a write on a stale
/// revision, the reads of an item and of a binding, a keyed create answered
/// with the item an earlier one made, and a link between two items.
#[tokio::test]
async fn nothing_is_told_for_a_refused_write_a_read_or_a_write_that_advances_no_revision() {
    let (service, recorder) = observed_service();
    let goal = goal_for(&service, BOUND_SESSION, "pursue").await;
    let other = created(&service, "another item").await;
    recorder.taken();

    let stale: UpdateWorkItemRequest = serde_json::from_value(json!({
        "id": other.id,
        "expected_revision": other.revision + 7,
        "description": "never written",
    }))
    .expect("an update");
    service
        .update(stale)
        .await
        .expect_err("a stale revision is refused");
    service
        .get(None, None, other.id.clone())
        .await
        .expect("read the item");
    service
        .attention_binding(AttentionBindingRequest {
            binding_id: goal.attention.binding_id.clone(),
            realm_id: None,
            namespace: None,
        })
        .await
        .expect("read the binding");
    service
        .list_attention(AttentionListRequest::default())
        .await
        .expect("list the bindings");
    assert_eq!(recorder.taken(), Vec::new(), "a refusal and reads");

    service
        .link(LinkWorkItemsRequest {
            realm_id: None,
            namespace: None,
            kind: WorkEdgeKind::Related,
            from_id: other.id.clone(),
            to_id: goal.item.id.clone(),
        })
        .await
        .expect("link the two items");
    assert_eq!(recorder.taken(), Vec::new(), "a link");

    let key = WorkAdmissionKey::new("one-key").expect("an admission key");
    let request = || CreateWorkItemRequest {
        title: "a keyed item".to_string(),
        ..Default::default()
    };
    service
        .create_idempotent(key.clone(), request())
        .await
        .expect("the first keyed create");
    assert_eq!(recorder.taken().len(), 1, "the keyed create is told once");
    service
        .create_idempotent(key, request())
        .await
        .expect("the same keyed create again");
    assert_eq!(
        recorder.taken(),
        Vec::new(),
        "the answer with the item an earlier create made writes nothing"
    );
}

/// The origin of a notice is the origin of the service value the write was
/// made through. A copy that was given a member's key tells that member,
/// the value it was copied from tells nobody, and both tell one observer.
#[tokio::test]
async fn a_notice_carries_the_origin_of_the_service_value_that_wrote() {
    let (service, recorder) = observed_service();
    let member = WorkOwnerKey::agent("member-1").expect("owner key");
    let members = service
        .clone()
        .with_commit_origin(WorkGraphCommitOrigin::Member(member.clone()));

    let by_member = created(&members, "the member's item").await;
    assert_eq!(
        recorder.taken(),
        vec![of_item(&by_member, WorkGraphCommitOrigin::Member(member))]
    );

    let by_host = created(&service, "the host's item").await;
    assert_eq!(
        recorder.taken(),
        vec![of_item(&by_host, WorkGraphCommitOrigin::Unattributed)]
    );
}
