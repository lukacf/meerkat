#![cfg(all(feature = "runtime-adapter", not(target_arch = "wasm32")))]
//! Idle pursuit rows: with pursuit attached to a mob, a committed write to
//! a bound item or to its binding starts one turn of the bound member
//! under the binding's current projection, and nothing else does.
//!
//! The members are turn-driven probes that record every model call with
//! its session and its turn overlay. Each mob has a second, sentinel
//! member bound to an item of its own: its turn after a write to that item
//! shows that every write told before it was weighed.

use super::*;
use std::collections::HashSet;

const WAIT: Duration = Duration::from_secs(30);

/// Keeps every notice it is given, in order.
#[derive(Default)]
struct RecordedNotices {
    notices: std::sync::Mutex<Vec<meerkat::WorkGraphCommitNotice>>,
}

impl RecordedNotices {
    fn taken(&self) -> Vec<meerkat::WorkGraphCommitNotice> {
        std::mem::take(&mut *self.notices.lock().expect("recorded notices"))
    }
}

impl meerkat::WorkGraphCommitObserver for RecordedNotices {
    fn committed(&self, notice: meerkat::WorkGraphCommitNotice) {
        self.notices.lock().expect("recorded notices").push(notice);
    }
}

type KeptTools = Arc<std::sync::Mutex<Vec<Arc<dyn meerkat_core::AgentToolDispatcher>>>>;

/// The gated probe builder, keeping the WorkGraph tools each member's
/// build was given.
struct ToolsKeepingProbeBuilder {
    inner: GatedOverlayProbeSessionAgentBuilder,
    kept: KeptTools,
}

#[async_trait]
impl SessionAgentBuilder for ToolsKeepingProbeBuilder {
    type Agent = OverlayProbeSessionAgent;

    fn session_comms_identity_root(&self) -> meerkat_core::SessionCommsIdentityRoot {
        self.inner.session_comms_identity_root()
    }

    async fn build_agent(
        &self,
        req: &CreateSessionRequest,
        event_tx: tokio::sync::mpsc::Sender<AgentEvent>,
    ) -> Result<Self::Agent, SessionError> {
        if let Some(tools) = req
            .build
            .as_ref()
            .and_then(|build| build.workgraph_tools.clone())
        {
            self.kept.lock().expect("kept tools").push(tools);
        }
        self.inner.build_agent(req, event_tx).await
    }
}

/// A mob of probe members over `host_workgraph`, and what its members'
/// model calls and builds recorded.
struct ProbeMob {
    handle: MobHandle,
    /// The mob's realm of the host's WorkGraph: the host writes through it.
    workgraph: meerkat::WorkGraphService,
    gate: Arc<TurnGate>,
    call_sessions: Arc<std::sync::Mutex<Vec<SessionId>>>,
    call_overlays: Arc<std::sync::Mutex<Vec<Option<TurnToolOverlay>>>>,
    kept_tools: KeptTools,
}

impl ProbeMob {
    async fn new(label: &str, host_workgraph: &meerkat::WorkGraphService) -> Self {
        let mut definition = with_unique_mob_id(sample_definition(), label);
        // A row gives a member a prompt as a host does.
        definition
            .profiles
            .get_mut(&ProfileName::from("worker"))
            .and_then(ProfileBinding::as_inline_mut)
            .expect("worker profile")
            .external_addressable = true;
        let gate = Arc::new(TurnGate::default());
        let call_sessions = Arc::new(std::sync::Mutex::new(Vec::new()));
        let call_overlays = Arc::new(std::sync::Mutex::new(Vec::new()));
        let kept_tools = KeptTools::default();
        let session_service = Arc::new(meerkat_session::EphemeralSessionService::new(
            ToolsKeepingProbeBuilder {
                inner: GatedOverlayProbeSessionAgentBuilder {
                    inner: OverlayProbeSessionAgentBuilder {
                        provider_visible_tools: Arc::default(),
                        provider_turn_overlays: Arc::clone(&call_overlays),
                        provider_call_sessions: Arc::clone(&call_sessions),
                    },
                    gate: Arc::clone(&gate),
                },
                kept: Arc::clone(&kept_tools),
            },
            16,
        ));
        let workgraph = crate::mob_scoped_workgraph_service(host_workgraph, &definition.id)
            .expect("mob realm")
            .expect("the mob's WorkGraph service");
        let handle = MobBuilder::new(definition, MobStorage::in_memory())
            .with_session_service(session_service)
            .with_workgraph_service(Some(host_workgraph.clone()))
            .allow_ephemeral_sessions(true)
            .create()
            .await
            .expect("create the probe mob");
        Self {
            handle,
            workgraph,
            gate,
            call_sessions,
            call_overlays,
            kept_tools,
        }
    }

    /// Spawn one turn-driven member and answer its identity and session.
    async fn spawn(&self, name: &str) -> (AgentIdentity, SessionId) {
        let identity = AgentIdentity::from(name);
        let mut spec = SpawnMemberSpec::new(ProfileName::from("worker"), identity.clone());
        spec.runtime_mode = Some(crate::MobRuntimeMode::TurnDriven);
        self.handle
            .spawn_spec(spec)
            .await
            .expect("spawn a turn-driven member");
        let session = self
            .handle
            .resolve_bridge_session_id(&identity)
            .await
            .expect("a session-bound member");
        (identity, session)
    }

    fn owner_target(&self, member: &AgentIdentity) -> meerkat::GoalAttentionTarget {
        crate::lower_agent_identity_attention_target(self.handle.mob_id(), member)
            .expect("the member's owner target")
    }

    /// The host makes a goal whose attention is bound to `target`.
    async fn goal(
        &self,
        title: &str,
        target: meerkat::GoalAttentionTarget,
    ) -> meerkat::GoalCreateResult {
        self.workgraph
            .create_goal(meerkat::GoalCreateRequest {
                failed_child_join_policy: Default::default(),
                cancelled_child_join_policy: Default::default(),
                priority: Default::default(),
                labels: Default::default(),
                due_at: None,
                not_before: None,
                snoozed_until: None,
                external_refs: Vec::new(),
                evidence_refs: Vec::new(),
                status: None,
                realm_id: None,
                namespace: None,
                title: title.to_string(),
                description: None,
                target,
                mode: meerkat::WorkAttentionMode::Coordinate,
                completion_policy: meerkat::WorkCompletionPolicy::SelfAttest,
                delegated_authority: meerkat::AttentionDelegatedAuthority::AddEvidence,
                projection_policy: meerkat::AttentionProjectionPolicy::default(),
            })
            .await
            .expect("the host makes the goal")
    }

    /// A revision of `item` written through `service`.
    async fn revise_through(
        &self,
        service: &meerkat::WorkGraphService,
        item: &meerkat::WorkItemId,
        text: &str,
    ) {
        let current = service
            .get(None, None, item.clone())
            .await
            .expect("read the item");
        let revision: meerkat::UpdateWorkItemRequest = serde_json::from_value(serde_json::json!({
            "id": item,
            "expected_revision": current.revision,
            "description": text,
        }))
        .expect("an item revision");
        service.update(revision).await.expect("revise the item");
    }

    /// The host revises `item`.
    async fn revise(&self, item: &meerkat::WorkItemId, text: &str) {
        self.revise_through(&self.workgraph, item, text).await;
    }

    async fn projection(
        &self,
        goal: &meerkat::GoalCreateResult,
    ) -> meerkat::AttentionContextProjection {
        self.workgraph
            .attention_projection(meerkat::AttentionProjectionRequest {
                binding_id: goal.attention.binding_id.clone(),
                realm_id: None,
                namespace: None,
            })
            .await
            .expect("the binding's projection")
            .projection
    }

    /// How often the model of the member on `session` was called.
    fn calls(&self, session: &SessionId) -> usize {
        self.call_sessions
            .lock()
            .expect("call sessions")
            .iter()
            .filter(|called| *called == session)
            .count()
    }

    /// Wait until the model of the member on `session` was called `count`
    /// times.
    async fn wait_calls(&self, session: &SessionId, count: usize) {
        tokio::time::timeout(WAIT, async {
            while self.calls(session) < count {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap_or_else(|_| {
            panic!(
                "the member on {session} was called {} time(s), {count} expected",
                self.calls(session)
            )
        });
    }

    /// The projection the last model call of the member on `session` ran
    /// under.
    fn last_projection(&self, session: &SessionId) -> Option<meerkat::AttentionContextProjection> {
        let sessions = self.call_sessions.lock().expect("call sessions");
        let overlays = self.call_overlays.lock().expect("call overlays");
        let last = sessions.iter().rposition(|called| called == session)?;
        let overlay = overlays.get(last)?.as_ref()?;
        meerkat::workgraph_attention_projection_from_overlay(Some(overlay))
            .expect("decode the attention projection of the turn's overlay")
    }
}

/// A probe mob with pursuit attached, and its sentinel member.
struct Pursued {
    mob: ProbeMob,
    pursuit: crate::WorkGraphPursuit,
    /// The targets the host's predicate says no to.
    not_pursued: Arc<std::sync::Mutex<HashSet<String>>>,
    sentinel_session: SessionId,
    sentinel_item: meerkat::WorkItemId,
}

impl Pursued {
    /// A mob whose pursuit is made and, with `attached`, attached. The
    /// sentinel and its goal are made before the pursuit is attached.
    async fn new(label: &str, attached: bool) -> Self {
        let not_pursued = Arc::new(std::sync::Mutex::new(HashSet::<String>::new()));
        let excluded = Arc::clone(&not_pursued);
        let pursuit = crate::WorkGraphPursuit::new(
            meerkat::WorkGraphService::with_scope(
                Arc::new(meerkat::MemoryWorkGraphStore::new()),
                "pursuit-rows-realm",
                meerkat::WorkNamespace::new("goals").expect("namespace"),
            ),
            Arc::new(move |binding: &meerkat::WorkAttentionBinding| {
                !excluded
                    .lock()
                    .expect("not pursued")
                    .contains(&binding.target.target_key())
            }),
        );
        let mob = ProbeMob::new(label, pursuit.workgraph_service()).await;
        let (sentinel, sentinel_session) = mob.spawn("sentinel").await;
        // Not pursued until the pursuit is attached: its creation must start
        // no turn in any row.
        let sentinel_target = mob.owner_target(&sentinel);
        not_pursued
            .lock()
            .expect("not pursued")
            .insert(sentinel_target.to_attention_target().target_key());
        let sentinel_item = mob
            .goal("the sentinel's goal", sentinel_target.clone())
            .await
            .item
            .id;
        let pursued = Self {
            mob,
            pursuit,
            not_pursued,
            sentinel_session,
            sentinel_item,
        };
        if attached {
            pursued.attach();
        }
        pursued
    }

    /// Attach the pursuit, and let the sentinel's binding be pursued from
    /// here on.
    fn attach(&self) {
        self.pursuit
            .attach(&self.mob.handle)
            .expect("attach the pursuit");
        self.not_pursued.lock().expect("not pursued").clear();
    }

    /// The host says the binding onto `target` is not pursued.
    fn do_not_pursue(&self, target: &meerkat::GoalAttentionTarget) {
        self.not_pursued
            .lock()
            .expect("not pursued")
            .insert(target.to_attention_target().target_key());
    }

    /// Wait until every write told so far was weighed: the sentinel takes a
    /// turn for a write made now, and a turn another member was handed
    /// meanwhile has had time to call its model.
    async fn settled(&self) {
        let before = self.mob.calls(&self.sentinel_session);
        self.mob
            .revise(&self.sentinel_item, &format!("settle {before}"))
            .await;
        self.mob
            .wait_calls(&self.sentinel_session, before + 1)
            .await;
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    /// Tell the pursuit once more of a write to `item` that the host made,
    /// through the observer of the host's service, with nothing written:
    /// the projection of every binding of the item is what it was.
    fn tell_again(&self, item: &meerkat::WorkItem) {
        self.pursuit
            .workgraph_service()
            .commit_observer()
            .expect("the pursuit's service has its observer")
            .committed(meerkat::WorkGraphCommitNotice {
                realm_id: item.realm_id.clone(),
                namespace: item.namespace.clone(),
                item_id: item.id.clone(),
                binding_id: None,
                origin: meerkat::WorkGraphCommitOrigin::Unattributed,
            });
    }
}

/// The host binds a goal to an idle member and later revises the item: the
/// member takes one turn for the goal's creation (a write of the item and
/// of the binding, told twice) and one for the revision, each under the
/// projection the store holds then, with no input given to the member by
/// anyone. Nothing else starts a turn: no time that passes, no write of
/// another item.
#[tokio::test]
async fn a_host_write_starts_one_turn_of_the_bound_idle_member_under_the_current_projection() {
    let pursued = Pursued::new("pursuit-one-turn", true).await;
    let (member, session) = pursued.mob.spawn("owner").await;
    let base = pursued.mob.calls(&session);

    let goal = pursued
        .mob
        .goal("work for the owner", pursued.mob.owner_target(&member))
        .await;
    pursued.mob.wait_calls(&session, base + 1).await;
    pursued.settled().await;
    assert_eq!(
        pursued.mob.calls(&session),
        base + 1,
        "a goal's creation, told for its item and for its binding, starts one turn"
    );
    assert_eq!(
        pursued.mob.last_projection(&session),
        Some(pursued.mob.projection(&goal).await),
        "the turn runs under the binding's current projection"
    );

    pursued
        .mob
        .revise(&goal.item.id, "revised by the host")
        .await;
    pursued.mob.wait_calls(&session, base + 2).await;
    pursued.settled().await;
    pursued.settled().await;
    assert_eq!(
        pursued.mob.calls(&session),
        base + 2,
        "a revision starts one turn, and nothing after it starts another"
    );
    assert_eq!(
        pursued.mob.last_projection(&session),
        Some(pursued.mob.projection(&goal).await),
        "the second turn runs under the projection of the revised item"
    );

    // The same write told again makes the same continuation: the member's
    // session, which took it once, takes no second turn for it.
    pursued.tell_again(&goal.item);
    pursued.settled().await;
    assert_eq!(
        pursued.mob.calls(&session),
        base + 2,
        "a write told again for an unchanged projection starts no turn"
    );
    pursued.mob.handle.shutdown().await.expect("shutdown");
}

/// Two revisions while the member's turn runs make two continuations and
/// both wait. When the turn ends they are taken up as one turn's inputs,
/// their two projections cannot both be the turn's, and neither runs: the
/// member takes no turn for either revision. It is not stuck: the next
/// write of the item starts one turn under the projection the store holds
/// then.
#[tokio::test]
async fn two_revisions_while_the_member_runs_start_no_turn_and_the_next_write_starts_one() {
    let pursued = Pursued::new("pursuit-while-running", true).await;
    let (member, session) = pursued.mob.spawn("owner").await;
    let base = pursued.mob.calls(&session);

    pursued.mob.gate.armed.store(true, Ordering::SeqCst);
    let goal = pursued
        .mob
        .goal("work for the owner", pursued.mob.owner_target(&member))
        .await;
    tokio::time::timeout(WAIT, pursued.mob.gate.entered.notified())
        .await
        .expect("the member's first turn is running");
    pursued.mob.gate.armed.store(false, Ordering::SeqCst);

    pursued.mob.revise(&goal.item.id, "first revision").await;
    pursued.settled().await;
    pursued.mob.revise(&goal.item.id, "second revision").await;
    pursued.settled().await;
    assert_eq!(
        pursued.mob.calls(&session),
        base,
        "the held turn has not called its model yet, and nothing ran beside it"
    );

    pursued.mob.gate.release.notify_one();
    pursued.mob.wait_calls(&session, base + 1).await;
    pursued.settled().await;
    pursued.settled().await;
    assert_eq!(
        pursued.mob.calls(&session),
        base + 1,
        "only the held turn calls the model: neither waiting continuation runs"
    );

    pursued.mob.revise(&goal.item.id, "third revision").await;
    pursued.mob.wait_calls(&session, base + 2).await;
    pursued.settled().await;
    assert_eq!(
        pursued.mob.calls(&session),
        base + 2,
        "the next write of the item starts one turn"
    );
    assert_eq!(
        pursued.mob.last_projection(&session),
        Some(pursued.mob.projection(&goal).await),
        "that turn runs under the current projection"
    );
    pursued.mob.handle.shutdown().await.expect("shutdown");
}

/// A host's prompt and a continuation that both wait while the member's
/// turn runs are not taken up together when it ends: a prompt is taken up
/// alone, so the member takes two turns, one for the prompt and one for
/// the continuation. The row asserts the facts of one run as one value:
/// the model calls after the held turn, whether the last call ran under
/// the current projection, and how the prompt's turn ended.
#[tokio::test]
async fn a_prompt_and_a_continuation_that_wait_together_take_two_turns() {
    let pursued = Pursued::new("pursuit-beside-a-prompt", true).await;
    let (member, session) = pursued.mob.spawn("owner").await;
    let base = pursued.mob.calls(&session);

    pursued.mob.gate.armed.store(true, Ordering::SeqCst);
    let goal = pursued
        .mob
        .goal("work for the owner", pursued.mob.owner_target(&member))
        .await;
    tokio::time::timeout(WAIT, pursued.mob.gate.entered.notified())
        .await
        .expect("the member's first turn is running");
    pursued.mob.gate.armed.store(false, Ordering::SeqCst);

    let prompt = pursued
        .mob
        .handle
        .member(&member)
        .await
        .expect("member handle")
        .start_turn(
            ContentInput::Text("a question from the host".to_string()),
            HandlingMode::Queue,
            crate::MemberTurnOptions::new(),
            None,
        )
        .await
        .expect("the host's prompt is admitted and waits");
    pursued
        .mob
        .revise(&goal.item.id, "revised by the host")
        .await;
    pursued.settled().await;

    pursued.mob.gate.release.notify_one();
    let prompt_ended = tokio::time::timeout(WAIT, prompt.wait())
        .await
        .expect("the prompt's turn ends");
    pursued.settled().await;
    let calls_after_the_held_turn = pursued.mob.calls(&session) - base - 1;
    let last_ran_under_the_current_projection =
        pursued.mob.last_projection(&session) == Some(pursued.mob.projection(&goal).await);
    assert_eq!(
        (
            calls_after_the_held_turn,
            last_ran_under_the_current_projection,
            prompt_ended
                .as_ref()
                .map(|_| ())
                .map_err(ToString::to_string),
        ),
        (2, true, Ok(())),
        "the prompt and the continuation are two turns, the prompt's own and the continuation's"
    );
    pursued.mob.handle.shutdown().await.expect("shutdown");
}

/// A close of the item after a continuation was handed over and before it
/// is applied: the continuation calls no model.
#[tokio::test]
async fn a_close_before_a_continuation_is_applied_starts_no_turn() {
    let pursued = Pursued::new("pursuit-closed-before-apply", true).await;
    let (member, session) = pursued.mob.spawn("owner").await;
    let base = pursued.mob.calls(&session);

    pursued.mob.gate.armed.store(true, Ordering::SeqCst);
    let goal = pursued
        .mob
        .goal("work for the owner", pursued.mob.owner_target(&member))
        .await;
    tokio::time::timeout(WAIT, pursued.mob.gate.entered.notified())
        .await
        .expect("the member's first turn is running");
    pursued.mob.gate.armed.store(false, Ordering::SeqCst);

    pursued
        .mob
        .revise(&goal.item.id, "revised by the host")
        .await;
    pursued.settled().await;
    let current = pursued
        .mob
        .workgraph
        .get(None, None, goal.item.id.clone())
        .await
        .expect("read the item");
    pursued
        .mob
        .workgraph
        .close(meerkat::CloseWorkItemRequest {
            id: goal.item.id.clone(),
            realm_id: None,
            namespace: None,
            expected_revision: current.revision,
            status: meerkat::WorkStatus::Cancelled,
        })
        .await
        .expect("the host closes the item");
    pursued.settled().await;

    pursued.mob.gate.release.notify_one();
    pursued.mob.wait_calls(&session, base + 1).await;
    pursued.settled().await;
    pursued.settled().await;
    assert_eq!(
        pursued.mob.calls(&session),
        base + 1,
        "only the turn that ran before the close calls the model"
    );
    pursued.mob.handle.shutdown().await.expect("shutdown");
}

/// Attaching a pursuit hands nothing over for what the store already
/// holds: a goal bound before starts no turn at the attach, the next write
/// of its item starts one, and attaching again starts none.
#[tokio::test]
async fn attaching_starts_no_turn_for_what_the_store_already_holds() {
    let pursued = Pursued::new("pursuit-attach", false).await;
    let (member, session) = pursued.mob.spawn("owner").await;
    let base = pursued.mob.calls(&session);
    let goal = pursued
        .mob
        .goal("bound before the attach", pursued.mob.owner_target(&member))
        .await;

    pursued.attach();
    pursued.settled().await;
    assert_eq!(
        pursued.mob.calls(&session),
        base,
        "the attach starts no turn for a goal bound before it"
    );

    pursued
        .mob
        .revise(&goal.item.id, "revised by the host")
        .await;
    pursued.mob.wait_calls(&session, base + 1).await;
    pursued.attach();
    pursued.settled().await;
    assert_eq!(
        pursued.mob.calls(&session),
        base + 1,
        "attaching again starts no second turn"
    );
    pursued.mob.handle.shutdown().await.expect("shutdown");
}

/// A binding the host's predicate says no to is not pursued: a write of its
/// item starts no turn of its member, while a pursued binding's member
/// takes its turn.
#[tokio::test]
async fn a_binding_the_host_does_not_state_is_not_pursued() {
    let pursued = Pursued::new("pursuit-not-stated", true).await;
    let (stated, stated_session) = pursued.mob.spawn("stated").await;
    let (unstated, unstated_session) = pursued.mob.spawn("unstated").await;
    let unstated_target = pursued.mob.owner_target(&unstated);
    pursued.do_not_pursue(&unstated_target);
    let stated_base = pursued.mob.calls(&stated_session);
    let unstated_base = pursued.mob.calls(&unstated_session);

    let unstated_goal = pursued.mob.goal("not stated", unstated_target).await;
    pursued
        .mob
        .revise(&unstated_goal.item.id, "revised by the host")
        .await;
    pursued
        .mob
        .goal("stated", pursued.mob.owner_target(&stated))
        .await;
    pursued
        .mob
        .wait_calls(&stated_session, stated_base + 1)
        .await;
    pursued.settled().await;
    assert_eq!(
        pursued.mob.calls(&unstated_session),
        unstated_base,
        "a binding that is not stated starts no turn"
    );
    assert_eq!(pursued.mob.calls(&stated_session), stated_base + 1);
    pursued.mob.handle.shutdown().await.expect("shutdown");
}

/// Two members pursue one item. A write of the item by either of them,
/// through a WorkGraph service that carries its key, starts no turn of
/// either: they do not keep each other turning. A write by a member that
/// does not pursue the item starts a turn of both, and so does the host's.
#[tokio::test]
async fn a_write_by_a_member_that_pursues_the_item_starts_no_turn_of_any_pursuer() {
    let pursued = Pursued::new("pursuit-own-write", true).await;
    let (writer, writer_session) = pursued.mob.spawn("writer").await;
    let (other, other_session) = pursued.mob.spawn("other").await;
    let (outsider, _outsider_session) = pursued.mob.spawn("outsider").await;
    let goal = pursued
        .mob
        .goal("work of two members", pursued.mob.owner_target(&writer))
        .await;
    let bound: meerkat::GoalBindExistingRequest = serde_json::from_value(serde_json::json!({
        "item_id": goal.item.id,
        "expected_item_revision": goal.item.revision,
        "target": pursued.mob.owner_target(&other),
        "mode": "coordinate",
    }))
    .expect("a bind request");
    pursued
        .mob
        .workgraph
        .bind_goal_attention(bound)
        .await
        .expect("the host binds the other member to the item");
    pursued.settled().await;
    let writer_base = pursued.mob.calls(&writer_session);
    let other_base = pursued.mob.calls(&other_session);

    // The service each member's own WorkGraph tools write through.
    let own_service =
        |member: &AgentIdentity| {
            pursued.mob.workgraph.clone().with_commit_origin(
                meerkat::WorkGraphCommitOrigin::Member(
                    crate::lower_agent_identity_owner_key(pursued.mob.handle.mob_id(), member)
                        .expect("the member's key"),
                ),
            )
        };
    pursued
        .mob
        .revise_through(
            &own_service(&writer),
            &goal.item.id,
            "revised by the writer",
        )
        .await;
    pursued.settled().await;
    pursued
        .mob
        .revise_through(&own_service(&other), &goal.item.id, "revised by the other")
        .await;
    pursued.settled().await;
    assert_eq!(
        (
            pursued.mob.calls(&writer_session),
            pursued.mob.calls(&other_session)
        ),
        (writer_base, other_base),
        "a write by a member that pursues the item starts no turn of any pursuer"
    );

    pursued
        .mob
        .revise_through(
            &own_service(&outsider),
            &goal.item.id,
            "revised by a member that does not pursue the item",
        )
        .await;
    pursued
        .mob
        .wait_calls(&writer_session, writer_base + 1)
        .await;
    pursued.mob.wait_calls(&other_session, other_base + 1).await;
    pursued.settled().await;
    pursued
        .mob
        .revise(&goal.item.id, "revised by the host")
        .await;
    pursued
        .mob
        .wait_calls(&writer_session, writer_base + 2)
        .await;
    pursued.mob.wait_calls(&other_session, other_base + 2).await;
    pursued.settled().await;
    assert_eq!(
        (
            pursued.mob.calls(&writer_session),
            pursued.mob.calls(&other_session)
        ),
        (writer_base + 2, other_base + 2),
        "a write by a member that does not pursue the item, and the host's, each start \
         one turn of each pursuer"
    );

    // A paused binding is still its member's own: the writer's write of the
    // item starts no turn of the other pursuer while the writer's binding
    // is paused.
    let writers_binding = pursued
        .mob
        .workgraph
        .attention_binding(meerkat::AttentionBindingRequest {
            binding_id: goal.attention.binding_id.clone(),
            realm_id: None,
            namespace: None,
        })
        .await
        .expect("read the writer's binding")
        .attention;
    pursued
        .mob
        .workgraph
        .pause_attention(meerkat::AttentionPauseRequest {
            binding_id: writers_binding.binding_id.clone(),
            realm_id: None,
            namespace: None,
            expected_revision: writers_binding.machine_state.revision,
            until: None,
        })
        .await
        .expect("the host pauses the writer's binding");
    pursued.settled().await;
    pursued
        .mob
        .revise_through(
            &own_service(&writer),
            &goal.item.id,
            "revised by the writer while its binding is paused",
        )
        .await;
    pursued.settled().await;
    assert_eq!(
        (
            pursued.mob.calls(&writer_session),
            pursued.mob.calls(&other_session)
        ),
        (writer_base + 2, other_base + 2),
        "a paused binding of the writer still counts as its own"
    );
    pursued.mob.handle.shutdown().await.expect("shutdown");
}

/// A binding onto a member's session is pursued like one onto its name.
#[tokio::test]
async fn a_binding_onto_a_members_session_is_pursued() {
    let pursued = Pursued::new("pursuit-session-target", true).await;
    let (_member, session) = pursued.mob.spawn("owner").await;
    let base = pursued.mob.calls(&session);

    let goal = pursued
        .mob
        .goal(
            "bound to the member's session",
            meerkat::GoalAttentionTarget::Session {
                session_id: session.clone(),
            },
        )
        .await;
    pursued.mob.wait_calls(&session, base + 1).await;
    pursued.settled().await;
    assert_eq!(pursued.mob.calls(&session), base + 1);
    assert_eq!(
        pursued.mob.last_projection(&session),
        Some(pursued.mob.projection(&goal).await)
    );
    pursued.mob.handle.shutdown().await.expect("shutdown");
}

/// The pursuit is started by a committed write that is told to it and by
/// nothing else: its source is a commit observer, and it names no timer, no
/// interval and no sleep.
#[test]
fn the_pursuit_is_told_of_writes_and_keeps_no_clock() {
    let source = include_str!("../workgraph_pursuit.rs");
    assert!(
        source.contains("impl meerkat::WorkGraphCommitObserver for"),
        "the pursuit is told of committed writes by the service that made them"
    );
    for clock in ["tokio::time", "interval", "sleep", "Instant", "timeout"] {
        assert!(
            !source.contains(clock),
            "the pursuit's source names `{clock}`"
        );
    }
}

/// The mob's realm of a host's WorkGraph tells the host's observer: a
/// write through it is told once, attributed to nobody.
#[tokio::test]
async fn the_mobs_realm_of_a_hosts_workgraph_tells_the_hosts_observer() {
    let recorded = Arc::new(RecordedNotices::default());
    let host = meerkat::WorkGraphService::with_scope(
        Arc::new(meerkat::MemoryWorkGraphStore::new()),
        "pursuit-carry-realm",
        meerkat::WorkNamespace::new("goals").expect("namespace"),
    )
    .with_commit_observer(recorded.clone());
    let mob_id = MobId::from("pursuit-carry-mob");
    let mob_realm = crate::mob_scoped_workgraph_service(&host, &mob_id)
        .expect("mob realm")
        .expect("the mob's WorkGraph service");

    let item = mob_realm
        .create(meerkat::CreateWorkItemRequest {
            title: "an item of the mob's realm".to_string(),
            ..Default::default()
        })
        .await
        .expect("create the item");
    assert_eq!(
        recorded.taken(),
        vec![meerkat::WorkGraphCommitNotice {
            realm_id: item.realm_id.clone(),
            namespace: item.namespace.clone(),
            item_id: item.id.clone(),
            binding_id: None,
            origin: meerkat::WorkGraphCommitOrigin::Unattributed,
        }]
    );
    assert_ne!(
        item.realm_id,
        host.default_realm_id(),
        "the item is in the mob's realm, not the host's"
    );
}

/// A write a member makes through the WorkGraph tools its build was given
/// is told with that member's key as its origin.
#[tokio::test]
async fn a_members_workgraph_tools_tell_the_member_as_the_origin() {
    let recorded = Arc::new(RecordedNotices::default());
    let host = meerkat::WorkGraphService::with_scope(
        Arc::new(meerkat::MemoryWorkGraphStore::new()),
        "pursuit-origin-realm",
        meerkat::WorkNamespace::new("goals").expect("namespace"),
    )
    .with_commit_observer(recorded.clone());
    let mob = ProbeMob::new("pursuit-origin", &host).await;
    let (member, _session) = mob.spawn("writer").await;
    let tools = mob
        .kept_tools
        .lock()
        .expect("kept tools")
        .last()
        .cloned()
        .expect("the member's build was given WorkGraph tools");
    recorded.taken();

    let args =
        RawValue::from_string(serde_json::json!({ "title": "made by the member" }).to_string())
            .expect("raw workgraph_create args");
    tools
        .dispatch(ToolCallView {
            id: "member-create",
            name: "workgraph_create",
            args: &args,
        })
        .await
        .expect("the member creates an item with its own tools");

    let told = recorded.taken();
    assert_eq!(told.len(), 1, "one item was written: {told:?}");
    assert_eq!(
        told[0].origin,
        meerkat::WorkGraphCommitOrigin::Member(
            crate::lower_agent_identity_owner_key(mob.handle.mob_id(), &member)
                .expect("the member's key")
        ),
        "the write is attributed to the member whose tools made it"
    );
    mob.handle.shutdown().await.expect("shutdown");
}

/// A member whose own binding on an item the host does not state is not a
/// pursuer of the item: its write of the item, through a WorkGraph service
/// that carries its key, starts one turn of the member whose binding the
/// host states, and none of its own.
#[tokio::test]
async fn a_write_by_a_member_whose_own_binding_is_not_stated_starts_the_stated_pursuers_turn() {
    let pursued = Pursued::new("pursuit-unstated-writer", true).await;
    let (coordinator, coordinator_session) = pursued.mob.spawn("coordinator").await;
    let (worker, worker_session) = pursued.mob.spawn("worker").await;
    // The host states the coordinator's binding only.
    let worker_target = pursued.mob.owner_target(&worker);
    pursued.do_not_pursue(&worker_target);
    let goal = pursued
        .mob
        .goal(
            "work of a coordinator and a worker",
            pursued.mob.owner_target(&coordinator),
        )
        .await;
    let bound: meerkat::GoalBindExistingRequest = serde_json::from_value(serde_json::json!({
        "item_id": goal.item.id,
        "expected_item_revision": goal.item.revision,
        "target": worker_target,
        "mode": "coordinate",
    }))
    .expect("a bind request");
    pursued
        .mob
        .workgraph
        .bind_goal_attention(bound)
        .await
        .expect("the host binds the worker to the item");
    pursued.settled().await;
    let coordinator_base = pursued.mob.calls(&coordinator_session);
    let worker_base = pursued.mob.calls(&worker_session);

    // The service the worker's own WorkGraph tools write through.
    let workers_own =
        pursued
            .mob
            .workgraph
            .clone()
            .with_commit_origin(meerkat::WorkGraphCommitOrigin::Member(
                crate::lower_agent_identity_owner_key(pursued.mob.handle.mob_id(), &worker)
                    .expect("the worker's key"),
            ));
    pursued
        .mob
        .revise_through(&workers_own, &goal.item.id, "revised by the worker")
        .await;
    pursued
        .mob
        .wait_calls(&coordinator_session, coordinator_base + 1)
        .await;
    pursued.settled().await;
    assert_eq!(
        (
            pursued.mob.calls(&coordinator_session),
            pursued.mob.calls(&worker_session)
        ),
        (coordinator_base + 1, worker_base),
        "the write of a member whose own binding is not stated starts one turn of the \
         stated pursuer and none of its own"
    );
    assert_eq!(
        pursued.mob.last_projection(&coordinator_session),
        Some(pursued.mob.projection(&goal).await),
        "the coordinator's turn runs under the projection of the item as the worker left it"
    );
    pursued.mob.handle.shutdown().await.expect("shutdown");
}
