//! TUX runtime composition — canonical per-target runtime registry.

use std::collections::HashMap;

use super::tux_runtime;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct State {
    pub targets: HashMap<String, tux_runtime::State>,
}

impl State {
    pub fn apply(&mut self, target: &str, event: tux_runtime::Event) {
        let state = self.targets.remove(target).unwrap_or_default();
        self.targets
            .insert(target.to_string(), tux_runtime::transition(state, event));
    }

    pub fn phase(&self, target: &str) -> tux_runtime::Phase {
        self.targets
            .get(target)
            .map(|state| state.phase)
            .unwrap_or(tux_runtime::Phase::Idle)
    }

    pub fn is_busy(&self, target: &str) -> bool {
        self.targets
            .get(target)
            .is_some_and(tux_runtime::State::is_busy)
    }

    pub fn last_activity_ms(&self, target: &str) -> Option<i64> {
        self.targets
            .get(target)
            .and_then(|state| state.last_activity_ms)
    }

    pub fn target_ids(&self) -> Vec<String> {
        self.targets.keys().cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::machines::tux_runtime::{Event, Phase};

    #[test]
    fn registry_tracks_targets_independently() {
        let mut state = State::default();
        state.apply("a", Event::DispatchRequested { now_ms: 10 });
        state.apply("b", Event::TargetRegistered { now_ms: 20 });
        assert_eq!(state.phase("a"), Phase::Dispatching);
        assert_eq!(state.phase("b"), Phase::Idle);
        assert!(state.is_busy("a"));
        assert!(!state.is_busy("b"));
        assert_eq!(state.last_activity_ms("b"), Some(20));
    }
}
