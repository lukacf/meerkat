//! Idle pursuit of bound WorkGraph work.
//!
//! A member whose attention is bound to a WorkGraph item takes a turn under
//! the binding's projection when somebody starts one. With pursuit attached
//! to a mob, a committed write to the item or to the binding itself starts
//! one: the service that made the write tells this module, and the member
//! the binding targets is handed a continuation for a turn under the
//! binding's current projection.
//!
//! What starts a turn, and what does not:
//!
//! - only a committed write of the mob's realm, told by the service that
//!   made it. Nothing here keeps a clock or reads the store on its own
//!   account: a mob that is started hands nothing over until the next
//!   write, and a window that opens in time (a pause that runs out, a
//!   snooze) starts no turn;
//! - a write to an item starts a turn for each pursuing binding of the
//!   item; a write to a binding starts one for that binding alone; a write
//!   that is told for both (a goal's creation) starts one turn, because
//!   the same projection handed over a second time is the same
//!   continuation to the member's session, which takes it once;
//! - a write made by a member that itself pursues the item, through its
//!   own WorkGraph tools, starts none, for that member or for another
//!   member that pursues the item: two pursuers of one item do not keep
//!   each other turning. Two members that each write the item the other
//!   pursues still can. A member whose own binding on the item the host
//!   does not state is not a pursuer: its write starts the turns of those
//!   that are. The rule reads who wrote, not what was written, so it
//!   holds for a write to a binding too: the one member tool that writes
//!   a binding hands the caller's own binding on to another target, the
//!   caller's binding ends with that write, and the new target takes its
//!   turn (unless the caller holds another pursued binding on the item);
//! - only a binding the host's predicate says is pursued, that is active,
//!   and whose target names a member of this mob.
//!
//! A continuation that cannot be handed over is logged with the refusal it
//! was answered with. Nothing is kept here of what was handed over or
//! refused: each write is weighed anew against the store, and whether a
//! continuation is one the member already took is its session's to say.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use tokio::sync::mpsc;

use crate::error::MobError;
use crate::ids::AgentIdentity;

use super::handle::MobHandle;

/// Says, for the host, whether an attention binding is pursued when its
/// work changes. The host decides: nothing a member passes is read.
pub type WorkGraphPursuitPredicate =
    Arc<dyn Fn(&meerkat::WorkAttentionBinding) -> bool + Send + Sync>;

type NoticeSender = mpsc::UnboundedSender<meerkat::WorkGraphCommitNotice>;

/// The notices of each attached mob's realm go to that mob's pursuit.
#[derive(Default)]
struct PursuitRoutes {
    by_realm: Mutex<HashMap<String, NoticeSender>>,
}

impl PursuitRoutes {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, NoticeSender>> {
        self.by_realm.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl meerkat::WorkGraphCommitObserver for PursuitRoutes {
    /// Called on the path of the write: it only passes the notice on.
    fn committed(&self, notice: meerkat::WorkGraphCommitNotice) {
        let route = self.lock().get(&notice.realm_id).cloned();
        if let Some(route) = route {
            // A pursuit that ended takes no more notices.
            let _ = route.send(notice);
        }
    }
}

/// Idle pursuit for the mobs of one host over one WorkGraph.
///
/// A host makes it from its WorkGraph service and its predicate, builds its
/// mobs with [`Self::workgraph_service`] and makes its own writes through
/// that service, and attaches each mob once it is created
/// ([`Self::attach`]). A host that makes none has no idle pursuit.
#[derive(Clone)]
pub struct WorkGraphPursuit {
    host_service: meerkat::WorkGraphService,
    pursues: WorkGraphPursuitPredicate,
    routes: Arc<PursuitRoutes>,
}

impl WorkGraphPursuit {
    /// Pursuit over `host_service`, for the bindings `pursues` says yes to.
    pub fn new(
        host_service: meerkat::WorkGraphService,
        pursues: WorkGraphPursuitPredicate,
    ) -> Self {
        let routes = Arc::new(PursuitRoutes::default());
        let observer: Arc<dyn meerkat::WorkGraphCommitObserver> = routes.clone();
        Self {
            host_service: host_service.with_commit_observer(observer),
            pursues,
            routes,
        }
    }

    /// The host's WorkGraph service with this pursuit told of its writes.
    /// The host gives it to its mobs and writes through it: a write made
    /// through another value of the service is not told.
    pub fn workgraph_service(&self) -> &meerkat::WorkGraphService {
        &self.host_service
    }

    /// Pursue for `mob` from now on, until the mob closes its admission.
    /// Nothing is handed over for what the store already holds: the next
    /// committed write of the mob's realm is the first one weighed. A mob
    /// attached again replaces its earlier pursuit.
    pub fn attach(&self, mob: &MobHandle) -> Result<(), MobError> {
        let service = super::mob_scoped_workgraph_service(&self.host_service, mob.mob_id())?
            .ok_or_else(|| {
                MobError::WiringError(format!(
                    "mob '{}' has no WorkGraph to pursue work of",
                    mob.mob_id()
                ))
            })?;
        let (route, notices) = mpsc::unbounded_channel();
        self.routes
            .lock()
            .insert(service.default_realm_id().to_string(), route);
        tokio::spawn(pursue(
            mob.observer(),
            service,
            Arc::clone(&self.pursues),
            notices,
            Arc::clone(&self.routes),
        ));
        Ok(())
    }
}

/// One mob's pursuit: each notice of its realm, in the order the writes
/// were told, until the mob closes its admission or is attached again.
async fn pursue(
    mob: MobHandle,
    service: meerkat::WorkGraphService,
    pursues: WorkGraphPursuitPredicate,
    mut notices: mpsc::UnboundedReceiver<meerkat::WorkGraphCommitNotice>,
    routes: Arc<PursuitRoutes>,
) {
    let mut closed = mob.admission_closed();
    loop {
        let notice = tokio::select! {
            notice = notices.recv() => match notice {
                Some(notice) => notice,
                None => break,
            },
            _ = closed.wait_for(Option::is_some) => break,
        };
        hand_over_for(&mob, &service, &pursues, notice).await;
    }
    drop(notices);
    routes.lock().retain(|_, route| !route.is_closed());
}

/// Weigh one committed write: hand a continuation to the member of each
/// binding it warrants a turn for.
async fn hand_over_for(
    mob: &MobHandle,
    service: &meerkat::WorkGraphService,
    pursues: &WorkGraphPursuitPredicate,
    notice: meerkat::WorkGraphCommitNotice,
) {
    if &notice.namespace != service.default_namespace() {
        return;
    }
    let live = match live_bindings(service).await {
        Ok(live) => live,
        Err(error) => {
            tracing::warn!(
                mob_id = %mob.mob_id(),
                item_id = %notice.item_id,
                %error,
                "the attention bindings could not be read after a WorkGraph write; no turn is started for it"
            );
            return;
        }
    };
    // The pursued bindings of the written item that are live (active, or
    // paused), each with the member of this mob it targets.
    let mut pursued = Vec::new();
    for binding in live {
        if binding.work_ref.item_id != notice.item_id || !pursues(&binding) {
            continue;
        }
        let Some(member) = bound_member(mob, &binding.target).await else {
            continue;
        };
        pursued.push((binding, member));
    }
    // A member that pursues the item starts no turn by writing it, for
    // itself or for another member that pursues it: two pursuers of one
    // item would otherwise keep each other turning for as long as both
    // write. A paused binding of the writer counts as its own. A host's
    // write, and a write by a member that does not pursue the item, start
    // a turn for each.
    if pursued
        .iter()
        .any(|(_, member)| written_by(&notice.origin, mob, member))
    {
        return;
    }
    for (binding, member) in pursued {
        // A write to a binding is weighed for that binding alone.
        if notice
            .binding_id
            .as_ref()
            .is_some_and(|written| written != &binding.binding_id)
        {
            continue;
        }
        // A paused binding has no projection: it takes no turn.
        let Some(projection) = pursuable_projection(service, &binding).await else {
            continue;
        };
        let continuation = match meerkat::surface::workgraph_attention_continuation(&projection) {
            Ok(continuation) => continuation,
            Err(error) => {
                tracing::warn!(
                    mob_id = %mob.mob_id(),
                    member = %member,
                    binding_id = %binding.binding_id,
                    %error,
                    "no continuation could be made from a binding's projection; no turn is started for it"
                );
                continue;
            }
        };
        // One write may be told twice for a binding (a goal's creation is a
        // write of its item and of its binding): the second hand-over is
        // the same continuation, and the member's session takes it once.
        // The turn's end is the session's own too: nobody waits for it
        // here, and a refusal leaves nothing behind.
        if let Err(error) = mob
            .start_attention_continuation(&member, continuation)
            .await
        {
            tracing::warn!(
                mob_id = %mob.mob_id(),
                member = %member,
                binding_id = %binding.binding_id,
                %error,
                "a WorkGraph attention continuation was refused; no turn is started for it"
            );
        }
    }
}

/// The live attention bindings of the service's namespace: the active ones
/// and the paused ones. A stopped or superseded binding is no member's any
/// more.
async fn live_bindings(
    service: &meerkat::WorkGraphService,
) -> Result<Vec<meerkat::WorkAttentionBinding>, meerkat::WorkGraphError> {
    let mut live = Vec::new();
    for status in [
        meerkat::WorkAttentionStatus::Active,
        meerkat::WorkAttentionStatus::Paused { until: None },
    ] {
        let listed = service
            .list_attention(meerkat::AttentionListRequest {
                realm_id: None,
                namespace: None,
                target: None,
                status: Some(status),
            })
            .await?;
        live.extend(listed.attention);
    }
    Ok(live)
}

/// The member of this mob a binding's target names: by its owner key, or by
/// the session a member of the roster holds.
async fn bound_member(
    mob: &MobHandle,
    target: &meerkat::WorkAttentionTarget,
) -> Option<AgentIdentity> {
    match target {
        meerkat::WorkAttentionTarget::LoweredOwner { owner_key } => {
            let (mob_id, member) = meerkat::mob_agent_owner_id_parts(&owner_key.id)?;
            (owner_key.kind == meerkat::WorkOwnerKind::Agent && mob_id == mob.mob_id().as_str())
                .then(|| AgentIdentity::from(member))
        }
        meerkat::WorkAttentionTarget::Session { session_id } => mob
            .roster
            .read()
            .await
            .list_all()
            .find(|entry| entry.member_ref.bridge_session_id() == Some(session_id))
            .map(|entry| entry.agent_identity.clone()),
    }
}

/// Whether the write was made by `member` itself, through its own
/// WorkGraph tools.
fn written_by(
    origin: &meerkat::WorkGraphCommitOrigin,
    mob: &MobHandle,
    member: &AgentIdentity,
) -> bool {
    match origin {
        meerkat::WorkGraphCommitOrigin::Unattributed => false,
        meerkat::WorkGraphCommitOrigin::Member(writer) => {
            crate::lower_agent_identity_owner_key(mob.mob_id(), member)
                .is_ok_and(|own| &own == writer)
        }
    }
}

/// Whether the binding's work may be pursued now, with the projection a
/// turn for it runs under. The rule is the projection's own: the binding
/// is eligible and its item is not terminal.
async fn pursuable_projection(
    service: &meerkat::WorkGraphService,
    binding: &meerkat::WorkAttentionBinding,
) -> Option<meerkat::AttentionContextProjection> {
    service
        .attention_projection(meerkat::AttentionProjectionRequest {
            binding_id: binding.binding_id.clone(),
            realm_id: None,
            namespace: None,
        })
        .await
        .ok()
        .map(|result| result.projection)
}
