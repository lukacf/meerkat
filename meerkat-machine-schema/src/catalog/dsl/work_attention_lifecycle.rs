use meerkat_machine_dsl::machine;

#[derive(
    Debug,
    Clone,
    Default,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    serde::Serialize,
    serde::Deserialize,
)]
pub struct WorkAttentionBindingKey(pub String);

machine! {
    machine WorkAttentionLifecycleMachine {
        version: 1,
        rust: "self" / "catalog::dsl::work_attention_lifecycle",

        state {
            lifecycle_phase: WorkAttentionLifecycleState,
            revision: u64,
            paused_until_utc_ms: Option<u64>,
            superseded_by_binding_key: Option<WorkAttentionBindingKey>,
            terminal_at_utc_ms: Option<u64>,
        }

        init(Active) {
            revision = 1,
            paused_until_utc_ms = None,
            superseded_by_binding_key = None,
            terminal_at_utc_ms = None,
        }

        terminal [Superseded, Stopped]

        phase WorkAttentionLifecycleState {
            Active,
            Paused,
            Superseded,
            Stopped,
        }

        input WorkAttentionLifecycleInput {
            Pause { expected_revision: u64, until_utc_ms: Option<u64> },
            Resume { expected_revision: u64 },
            Supersede { expected_revision: u64, superseded_by_binding_key: WorkAttentionBindingKey, at_utc_ms: u64 },
            Stop { expected_revision: u64, at_utc_ms: u64 },
        }

        effect WorkAttentionLifecycleEffect {
            AttentionPaused { revision: u64 },
            AttentionResumed { revision: u64 },
            AttentionSuperseded { revision: u64 },
            AttentionStopped { revision: u64 },
        }

        invariant live_has_no_terminal_time {
            (self.lifecycle_phase != Phase::Active && self.lifecycle_phase != Phase::Paused)
                || self.terminal_at_utc_ms == None
        }

        invariant paused_has_pause_state {
            self.lifecycle_phase == Phase::Paused || self.paused_until_utc_ms == None
        }

        invariant superseded_records_successor {
            self.lifecycle_phase != Phase::Superseded || self.superseded_by_binding_key != None
        }

        disposition AttentionPaused => local,
        disposition AttentionResumed => local,
        disposition AttentionSuperseded => local,
        disposition AttentionStopped => local,

        transition PauseActive {
            on input Pause { expected_revision, until_utc_ms }
            guard { self.lifecycle_phase == Phase::Active && self.revision == expected_revision }
            update {
                self.lifecycle_phase = Phase::Paused;
                self.paused_until_utc_ms = until_utc_ms;
                self.revision += 1;
            }
            to Paused
            emit AttentionPaused { revision: self.revision }
        }

        transition PausePaused {
            on input Pause { expected_revision, until_utc_ms }
            guard { self.lifecycle_phase == Phase::Paused && self.revision == expected_revision }
            update {
                self.paused_until_utc_ms = until_utc_ms;
                self.revision += 1;
            }
            to Paused
            emit AttentionPaused { revision: self.revision }
        }

        transition ResumePaused {
            on input Resume { expected_revision }
            guard { self.lifecycle_phase == Phase::Paused && self.revision == expected_revision }
            update {
                self.lifecycle_phase = Phase::Active;
                self.paused_until_utc_ms = None;
                self.revision += 1;
            }
            to Active
            emit AttentionResumed { revision: self.revision }
        }

        transition SupersedeActive {
            on input Supersede { expected_revision, superseded_by_binding_key, at_utc_ms }
            guard { self.lifecycle_phase == Phase::Active && self.revision == expected_revision }
            update {
                self.lifecycle_phase = Phase::Superseded;
                self.paused_until_utc_ms = None;
                self.superseded_by_binding_key = Some(superseded_by_binding_key);
                self.terminal_at_utc_ms = Some(at_utc_ms);
                self.revision += 1;
            }
            to Superseded
            emit AttentionSuperseded { revision: self.revision }
        }

        transition SupersedePaused {
            on input Supersede { expected_revision, superseded_by_binding_key, at_utc_ms }
            guard { self.lifecycle_phase == Phase::Paused && self.revision == expected_revision }
            update {
                self.lifecycle_phase = Phase::Superseded;
                self.paused_until_utc_ms = None;
                self.superseded_by_binding_key = Some(superseded_by_binding_key);
                self.terminal_at_utc_ms = Some(at_utc_ms);
                self.revision += 1;
            }
            to Superseded
            emit AttentionSuperseded { revision: self.revision }
        }

        transition StopActive {
            on input Stop { expected_revision, at_utc_ms }
            guard { self.lifecycle_phase == Phase::Active && self.revision == expected_revision }
            update {
                self.lifecycle_phase = Phase::Stopped;
                self.paused_until_utc_ms = None;
                self.terminal_at_utc_ms = Some(at_utc_ms);
                self.revision += 1;
            }
            to Stopped
            emit AttentionStopped { revision: self.revision }
        }

        transition StopPaused {
            on input Stop { expected_revision, at_utc_ms }
            guard { self.lifecycle_phase == Phase::Paused && self.revision == expected_revision }
            update {
                self.lifecycle_phase = Phase::Stopped;
                self.paused_until_utc_ms = None;
                self.terminal_at_utc_ms = Some(at_utc_ms);
                self.revision += 1;
            }
            to Stopped
            emit AttentionStopped { revision: self.revision }
        }
    }
}

impl serde::Serialize for WorkAttentionLifecycleState {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(match self {
            Self::Active => "active",
            Self::Paused => "paused",
            Self::Superseded => "superseded",
            Self::Stopped => "stopped",
        })
    }
}

impl<'de> serde::Deserialize<'de> for WorkAttentionLifecycleState {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = <String as serde::Deserialize>::deserialize(deserializer)?;
        match value.as_str() {
            "active" | "Active" => Ok(Self::Active),
            "paused" | "Paused" => Ok(Self::Paused),
            "superseded" | "Superseded" => Ok(Self::Superseded),
            "stopped" | "Stopped" => Ok(Self::Stopped),
            other => Err(serde::de::Error::custom(format!(
                "invalid WorkAttentionLifecycleState `{other}`"
            ))),
        }
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
struct WorkAttentionLifecycleMachineStateWire {
    lifecycle_phase: WorkAttentionLifecycleState,
    revision: u64,
    #[serde(default)]
    paused_until_utc_ms: Option<u64>,
    #[serde(default)]
    superseded_by_binding_key: Option<WorkAttentionBindingKey>,
    #[serde(default)]
    terminal_at_utc_ms: Option<u64>,
}

impl From<&WorkAttentionLifecycleMachineState> for WorkAttentionLifecycleMachineStateWire {
    fn from(state: &WorkAttentionLifecycleMachineState) -> Self {
        Self {
            lifecycle_phase: state.lifecycle_phase,
            revision: state.revision,
            paused_until_utc_ms: state.paused_until_utc_ms,
            superseded_by_binding_key: state.superseded_by_binding_key.clone(),
            terminal_at_utc_ms: state.terminal_at_utc_ms,
        }
    }
}

impl From<WorkAttentionLifecycleMachineStateWire> for WorkAttentionLifecycleMachineState {
    fn from(wire: WorkAttentionLifecycleMachineStateWire) -> Self {
        Self {
            lifecycle_phase: wire.lifecycle_phase,
            revision: wire.revision,
            paused_until_utc_ms: wire.paused_until_utc_ms,
            superseded_by_binding_key: wire.superseded_by_binding_key,
            terminal_at_utc_ms: wire.terminal_at_utc_ms,
        }
    }
}

impl serde::Serialize for WorkAttentionLifecycleMachineState {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        WorkAttentionLifecycleMachineStateWire::from(self).serialize(serializer)
    }
}

impl<'de> serde::Deserialize<'de> for WorkAttentionLifecycleMachineState {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        WorkAttentionLifecycleMachineStateWire::deserialize(deserializer).map(Self::from)
    }
}
