//! Who the helpers work for, which of them are running, and what the UI
//! hears about them.
//!
//! The registry is the TUI's own bookkeeping next to the mob runtime (which
//! owns the helpers): every session the TUI opens registers its model and
//! auth binding; every helper spawn is tagged with the session it works for
//! (its *owner*: the session whose user's words its calls are judged
//! against), its path of helper labels (nesting) and the model it runs on.

use std::collections::HashMap;

use indexmap::IndexMap;
use meerkat_core::{AgentEvent, AuthBindingRef, ScopedAgentEvent, SessionId, StreamScopeFrame};
use parking_lot::{Mutex, RwLock};
use tokio::sync::mpsc;

/// Prefix of the id of the implicit mob a session's `delegate` helpers run
/// in (`MobDefinition::implicit` names it `implicit-<session id>`).
pub const IMPLICIT_MOB_PREFIX: &str = "implicit-";

/// What a helper is and whom it works for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HelperTag {
    /// The TUI session the helper works for (directly, or through the
    /// helpers above it).
    pub owner: SessionId,
    /// Helper labels from the outermost helper to this one (its depth is the
    /// length).
    pub path: Vec<String>,
    /// The model the helper runs on.
    pub model: String,
}

impl HelperTag {
    /// The helper's own label (its member identity).
    #[must_use]
    pub fn label(&self) -> &str {
        self.path.last().map_or("", String::as_str)
    }

    /// Nesting depth (1 for a helper of the session's own agent).
    #[must_use]
    pub fn depth(&self) -> usize {
        self.path.len()
    }

    /// `event` scoped to this helper: the owner's primary frame, then one
    /// mob-member frame per helper on the path.
    #[must_use]
    pub fn scope(&self, event: AgentEvent) -> ScopedAgentEvent {
        let mut frames = Vec::with_capacity(self.path.len() + 1);
        frames.push(StreamScopeFrame::Primary {
            session_id: self.owner.to_string(),
        });
        frames.extend(self.path.iter().map(|label| StreamScopeFrame::MobMember {
            flow_run_id: String::new(),
            agent_identity: label.clone(),
            agent_runtime_id: None,
            fence_token: None,
            generation: None,
        }));
        ScopedAgentEvent::new(frames, event)
    }
}

/// A running helper, as the status bar lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HelperStatus {
    /// Member identity.
    pub label: String,
    /// The model it runs on.
    pub model: String,
    /// Nesting depth.
    pub depth: usize,
}

/// What the runner forwards to the UI.
#[derive(Debug, Clone)]
pub enum HelperUpdate {
    /// An event of a helper, scoped to it.
    Event(ScopedAgentEvent),
    /// The helpers running now changed.
    Live(Vec<HelperStatus>),
}

/// The session a new helper works for, and what it inherits from it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Owner {
    /// The TUI session.
    pub session: SessionId,
    /// Labels of the helpers between the session and the new helper.
    pub path: Vec<String>,
    /// The session's model.
    pub model: String,
    /// The session's explicit auth binding (a configured endpoint's).
    pub auth_binding: Option<AuthBindingRef>,
}

/// A TUI session's model, as helpers inherit it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct SessionModel {
    model: String,
    auth_binding: Option<AuthBindingRef>,
}

/// A helper's key: its mob and its member identity.
pub type HelperKey = (String, String);

/// The TUI's helper bookkeeping, shared by the spawn customizer, the event
/// followers, the session handles and the runner.
#[derive(Debug)]
pub struct HelperRegistry {
    /// TUI sessions, most recently opened last.
    sessions: RwLock<IndexMap<SessionId, SessionModel>>,
    /// Helper sessions (once known) and their tags.
    members: RwLock<HashMap<SessionId, HelperTag>>,
    /// Running helpers, in spawn order.
    live: Mutex<IndexMap<HelperKey, HelperTag>>,
    updates: mpsc::UnboundedSender<HelperUpdate>,
}

impl HelperRegistry {
    /// An empty registry and the receiver of its updates.
    #[must_use]
    pub fn new() -> (Self, mpsc::UnboundedReceiver<HelperUpdate>) {
        let (updates, rx) = mpsc::unbounded_channel();
        (
            Self {
                sessions: RwLock::new(IndexMap::new()),
                members: RwLock::new(HashMap::new()),
                live: Mutex::new(IndexMap::new()),
                updates,
            },
            rx,
        )
    }

    /// `session` (re)opened or switched to `model` on `auth_binding`: its
    /// helpers inherit both, and it becomes the most recent session.
    pub fn register_session(
        &self,
        session: &SessionId,
        model: &str,
        auth_binding: Option<&AuthBindingRef>,
    ) {
        let mut sessions = self.sessions.write();
        sessions.shift_remove(session);
        sessions.insert(
            session.clone(),
            SessionModel {
                model: model.to_string(),
                auth_binding: auth_binding.cloned(),
            },
        );
    }

    /// Whom a helper spawned into `mob_id` works for.
    ///
    /// A `delegate` helper runs in its delegating agent's implicit mob
    /// (`implicit-<session id>`): that session is a TUI session, or a helper
    /// whose owner the new helper inherits (nesting). A helper of any other
    /// mob (one an agent created with `mob_create`) works for the most
    /// recently opened session. `None` before any session is registered.
    #[must_use]
    pub fn owner_of(&self, mob_id: &str) -> Option<Owner> {
        let delegator = mob_id
            .strip_prefix(IMPLICIT_MOB_PREFIX)
            .and_then(|id| SessionId::parse(id).ok());
        let sessions = self.sessions.read();
        if let Some(delegator) = delegator {
            if let Some(model) = sessions.get(&delegator) {
                return Some(owner(&delegator, Vec::new(), model));
            }
            if let Some(tag) = self.members.read().get(&delegator) {
                let inherited = SessionModel {
                    model: tag.model.clone(),
                    auth_binding: sessions
                        .get(&tag.owner)
                        .and_then(|model| model.auth_binding.clone()),
                };
                return Some(owner(&tag.owner, tag.path.clone(), &inherited));
            }
        }
        sessions
            .last()
            .map(|(session, model)| owner(session, Vec::new(), model))
    }

    /// A helper is being spawned into `mob_id` as `identity`.
    pub fn spawning(&self, key: HelperKey, tag: HelperTag) {
        self.live.lock().insert(key, tag);
        self.publish_live();
    }

    /// The helper `key` runs as `session`: helpers it delegates to inherit
    /// its owner.
    pub fn bind_session(&self, session: SessionId, tag: &HelperTag) {
        self.members.write().insert(session, tag.clone());
    }

    /// The helper `key` is gone (retired, or never observable).
    pub fn finished(&self, key: &HelperKey) {
        if self.live.lock().shift_remove(key).is_some() {
            self.publish_live();
        }
    }

    /// Running helpers that work for `owner`.
    #[must_use]
    pub fn live_for(&self, owner: &SessionId) -> Vec<HelperKey> {
        self.live
            .lock()
            .iter()
            .filter(|(_, tag)| &tag.owner == owner)
            .map(|(key, _)| key.clone())
            .collect()
    }

    /// Running helpers.
    #[must_use]
    pub fn live(&self) -> Vec<HelperStatus> {
        self.live
            .lock()
            .values()
            .map(|tag| HelperStatus {
                label: tag.label().to_string(),
                model: tag.model.clone(),
                depth: tag.depth(),
            })
            .collect()
    }

    /// Forward a helper event to the UI.
    pub fn emit(&self, tag: &HelperTag, event: AgentEvent) {
        let _ = self.updates.send(HelperUpdate::Event(tag.scope(event)));
    }

    fn publish_live(&self) {
        let _ = self.updates.send(HelperUpdate::Live(self.live()));
    }
}

fn owner(session: &SessionId, path: Vec<String>, model: &SessionModel) -> Owner {
    Owner {
        session: session.clone(),
        path,
        model: model.model.clone(),
        auth_binding: model.auth_binding.clone(),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    fn binding() -> AuthBindingRef {
        AuthBindingRef {
            realm: meerkat_core::RealmId::parse(crate::endpoints::ENDPOINT_REALM).unwrap(),
            binding: meerkat_core::BindingId::parse("proxy").unwrap(),
            profile: None,
            origin: meerkat_core::BindingOrigin::Configured,
        }
    }

    #[test]
    fn owners_follow_implicit_mobs_and_nesting() {
        let (registry, mut rx) = HelperRegistry::new();
        assert!(registry.owner_of("implicit-x").is_none());
        let a = SessionId::new();
        let b = SessionId::new();
        registry.register_session(&a, "m-a", Some(&binding()));
        registry.register_session(&b, "m-b", None);
        // A session's own implicit mob.
        let owner = registry.owner_of(&format!("implicit-{a}")).unwrap();
        assert_eq!(owner.session, a);
        assert_eq!(owner.model, "m-a");
        assert_eq!(owner.auth_binding, Some(binding()));
        assert!(owner.path.is_empty());
        // Any other mob: the most recent session.
        assert_eq!(registry.owner_of("team").unwrap().session, b);
        registry.register_session(&a, "m-a2", Some(&binding()));
        assert_eq!(registry.owner_of("team").unwrap().model, "m-a2");

        // A helper of `a`, then a helper of that helper.
        let tag = HelperTag {
            owner: a.clone(),
            path: vec!["scout".into()],
            model: "m-a2".into(),
        };
        let key = (format!("implicit-{a}"), "scout".to_string());
        registry.spawning(key.clone(), tag.clone());
        let helper_session = SessionId::new();
        registry.bind_session(helper_session.clone(), &tag);
        let nested = registry
            .owner_of(&format!("implicit-{helper_session}"))
            .unwrap();
        assert_eq!(nested.session, a);
        assert_eq!(nested.path, vec!["scout".to_string()]);
        assert_eq!(nested.auth_binding, Some(binding()));
        assert_eq!(registry.live_for(&a), vec![key.clone()]);
        assert!(registry.live_for(&b).is_empty());
        registry.finished(&key);
        assert!(registry.live().is_empty());

        let mut lives = Vec::new();
        while let Ok(update) = rx.try_recv() {
            if let HelperUpdate::Live(live) = update {
                lives.push(live.len());
            }
        }
        assert_eq!(lives, vec![1, 0]);
    }

    #[test]
    fn helper_events_are_scoped_by_their_path() {
        let tag = HelperTag {
            owner: SessionId::new(),
            path: vec!["outer".into(), "inner".into()],
            model: "m".into(),
        };
        assert_eq!(tag.label(), "inner");
        assert_eq!(tag.depth(), 2);
        let scoped = tag.scope(AgentEvent::TextDelta {
            delta: "x".into(),
            assistant_message_id: None,
        });
        assert_eq!(scoped.scope_path.len(), 3);
        assert!(matches!(
            &scoped.scope_path[2],
            StreamScopeFrame::MobMember { agent_identity, .. } if agent_identity == "inner"
        ));
    }
}
