//! TUX runtime owner — authoritative per-target execution/liveness state.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Idle,
    Dispatching,
    Running,
    Stalled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct State {
    pub phase: Phase,
    pub last_activity_ms: Option<i64>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            phase: Phase::Idle,
            last_activity_ms: None,
        }
    }
}

impl State {
    pub fn is_busy(&self) -> bool {
        matches!(self.phase, Phase::Dispatching | Phase::Running)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    TargetRegistered { now_ms: i64 },
    LivenessObserved { now_ms: i64 },
    DispatchRequested { now_ms: i64 },
    ActivityObserved { now_ms: i64 },
    RunStarted { now_ms: i64 },
    RunCompleted { now_ms: i64 },
    RunFailed { now_ms: i64 },
    SendFailed { now_ms: i64 },
    TargetDisconnected { now_ms: i64 },
    TickStale { now_ms: i64, stale_after_ms: i64 },
}

pub fn transition(state: State, event: Event) -> State {
    match (state, event) {
        (_, Event::TargetRegistered { now_ms }) => State {
            phase: Phase::Idle,
            last_activity_ms: Some(now_ms),
        },
        (
            State {
                phase,
                last_activity_ms: _,
            },
            Event::LivenessObserved { now_ms },
        ) => State {
            phase,
            last_activity_ms: Some(now_ms),
        },
        (
            State {
                last_activity_ms, ..
            },
            Event::DispatchRequested { now_ms },
        ) => State {
            phase: Phase::Dispatching,
            last_activity_ms: Some(now_ms).or(last_activity_ms),
        },
        (_, Event::RunCompleted { now_ms } | Event::RunFailed { now_ms }) => State {
            phase: Phase::Idle,
            last_activity_ms: Some(now_ms),
        },
        (_, Event::SendFailed { now_ms } | Event::TargetDisconnected { now_ms }) => State {
            phase: Phase::Idle,
            last_activity_ms: Some(now_ms),
        },
        (_, Event::ActivityObserved { now_ms } | Event::RunStarted { now_ms }) => State {
            phase: Phase::Running,
            last_activity_ms: Some(now_ms),
        },
        (
            State {
                phase,
                last_activity_ms,
            },
            Event::TickStale {
                now_ms,
                stale_after_ms,
            },
        ) => {
            let should_stall = matches!(phase, Phase::Dispatching | Phase::Running)
                && last_activity_ms.is_some_and(|last| now_ms - last >= stale_after_ms);
            if should_stall {
                State {
                    phase: Phase::Stalled,
                    last_activity_ms,
                }
            } else {
                State {
                    phase,
                    last_activity_ms,
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dispatch_marks_target_busy() {
        let state = transition(State::default(), Event::DispatchRequested { now_ms: 10 });
        assert_eq!(state.phase, Phase::Dispatching);
        assert!(state.is_busy());
        assert_eq!(state.last_activity_ms, Some(10));
    }

    #[test]
    fn send_failure_clears_busy() {
        let state = transition(
            State {
                phase: Phase::Dispatching,
                last_activity_ms: Some(10),
            },
            Event::SendFailed { now_ms: 20 },
        );
        assert_eq!(state.phase, Phase::Idle);
        assert_eq!(state.last_activity_ms, Some(20));
    }

    #[test]
    fn activity_promotes_dispatch_to_running() {
        let state = transition(
            State {
                phase: Phase::Dispatching,
                last_activity_ms: Some(10),
            },
            Event::ActivityObserved { now_ms: 15 },
        );
        assert_eq!(state.phase, Phase::Running);
        assert_eq!(state.last_activity_ms, Some(15));
    }

    #[test]
    fn completion_returns_to_idle() {
        let state = transition(
            State {
                phase: Phase::Running,
                last_activity_ms: Some(15),
            },
            Event::RunCompleted { now_ms: 25 },
        );
        assert_eq!(state.phase, Phase::Idle);
        assert_eq!(state.last_activity_ms, Some(25));
    }

    #[test]
    fn disconnect_returns_to_idle() {
        let state = transition(
            State {
                phase: Phase::Running,
                last_activity_ms: Some(15),
            },
            Event::TargetDisconnected { now_ms: 40 },
        );
        assert_eq!(state.phase, Phase::Idle);
        assert_eq!(state.last_activity_ms, Some(40));
    }

    #[test]
    fn stale_tick_marks_runtime_stalled() {
        let state = transition(
            State {
                phase: Phase::Running,
                last_activity_ms: Some(10),
            },
            Event::TickStale {
                now_ms: 50,
                stale_after_ms: 30,
            },
        );
        assert_eq!(state.phase, Phase::Stalled);
        assert_eq!(state.last_activity_ms, Some(10));
    }

    #[test]
    fn fresh_activity_recovers_from_stalled() {
        let state = transition(
            State {
                phase: Phase::Stalled,
                last_activity_ms: Some(10),
            },
            Event::ActivityObserved { now_ms: 55 },
        );
        assert_eq!(state.phase, Phase::Running);
        assert_eq!(state.last_activity_ms, Some(55));
    }

    #[test]
    fn liveness_updates_do_not_reclassify_phase() {
        let state = transition(
            State {
                phase: Phase::Stalled,
                last_activity_ms: Some(10),
            },
            Event::LivenessObserved { now_ms: 12 },
        );
        assert_eq!(state.phase, Phase::Stalled);
        assert_eq!(state.last_activity_ms, Some(12));
        assert!(!state.is_busy());
    }
}
