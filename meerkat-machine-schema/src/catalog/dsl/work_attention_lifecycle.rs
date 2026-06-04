use super::OptionValueExt;
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

/// Attention stance the binding was provisioned with. A raw binding fact the
/// shell extracts (no shell decision); the machine owns the permission POLICY
/// that maps `(mode, delegated_authority)` to the projected authority verdict.
#[derive(
    Debug,
    Clone,
    Copy,
    Default,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    serde::Serialize,
    serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum WorkAttentionMode {
    #[default]
    Pursue,
    Coordinate,
    Review,
    Falsify,
    Judge,
    Observe,
}

/// Delegated closure authority the binding was provisioned with. A raw binding
/// fact the shell extracts; the machine owns the permission POLICY.
#[derive(
    Debug,
    Clone,
    Copy,
    Default,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    serde::Serialize,
    serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum AttentionDelegatedAuthority {
    #[default]
    AddEvidence,
    CloseOwnReviewItem,
    RequestClosure,
    CloseIfPolicyAllows,
}

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
            // Attention-projection eligibility classification. The shell extracts
            // only the raw wall-clock `now_utc_ms` (a pure observation) and feeds
            // it here; this machine owns the eligibility POLICY — including the
            // Paused deadline-elapsed rule (`paused_until <= now`) — over its own
            // lifecycle_phase + paused_until_utc_ms and emits the verdict. The
            // shell mirrors the emitted `AttentionEligibilityClassified.eligible`
            // and fails closed without a verdict; it decides nothing.
            ClassifyAttentionEligibility { now_utc_ms: u64 },
            // Projected-authority permission classification. The shell extracts
            // the raw binding facts (`mode`, `delegated_authority`); this machine
            // owns the COMPLETE per-stance tool-admission POLICY that maps them to
            // the projected authority capability bits and emits
            // `AttentionAuthorityClassified`. The attention-scoped tool surface is
            // a pure tool-name -> capability-bit decoder over the emitted bits
            // (no per-mode allow-set of its own) and fails closed without a
            // verdict.
            ClassifyAttentionAuthority {
                mode: Enum<WorkAttentionMode>,
                delegated_authority: Enum<AttentionDelegatedAuthority>,
            },
        }

        effect WorkAttentionLifecycleEffect {
            AttentionPaused { revision: u64 },
            AttentionResumed { revision: u64 },
            AttentionSuperseded { revision: u64 },
            AttentionStopped { revision: u64 },
            AttentionEligibilityClassified { eligible: bool },
            AttentionAuthorityClassified {
                can_get: bool,
                can_add_evidence: bool,
                can_release: bool,
                can_update: bool,
                can_block: bool,
                can_create: bool,
                can_link: bool,
                can_link_parent: bool,
                can_link_related: bool,
                can_link_derived_from: bool,
                can_close_own_review_item: bool,
                can_close_if_policy_allows: bool,
            },
        }

        // Adversarial stances must never gain closure authority over the item
        // they inspect; this is the core of the projected-authority POLICY.
        helper attention_is_adversarial(mode: WorkAttentionMode) -> bool {
            mode == WorkAttentionMode::Review
                || mode == WorkAttentionMode::Falsify
                || mode == WorkAttentionMode::Observe
        }

        // Reading the attention item is permitted in every stance, including the
        // read-only Observe stance.
        helper attention_can_get(mode: WorkAttentionMode) -> bool {
            mode == WorkAttentionMode::Pursue
                || mode == WorkAttentionMode::Coordinate
                || mode == WorkAttentionMode::Review
                || mode == WorkAttentionMode::Falsify
                || mode == WorkAttentionMode::Judge
                || mode == WorkAttentionMode::Observe
        }

        helper attention_can_add_evidence(mode: WorkAttentionMode) -> bool {
            mode != WorkAttentionMode::Observe
        }

        // Releasing the active claim on the attention item is a Pursue-stance
        // mutation only.
        helper attention_can_release(mode: WorkAttentionMode) -> bool {
            mode == WorkAttentionMode::Pursue
        }

        // Mutating the attention item's own fields is permitted while pursuing or
        // coordinating it.
        helper attention_can_update(mode: WorkAttentionMode) -> bool {
            mode == WorkAttentionMode::Pursue || mode == WorkAttentionMode::Coordinate
        }

        // Blocking the attention item is a Pursue-stance mutation only.
        helper attention_can_block(mode: WorkAttentionMode) -> bool {
            mode == WorkAttentionMode::Pursue
        }

        // Decomposing the attention item by creating child work is a
        // Coordinate-stance authority.
        helper attention_can_create(mode: WorkAttentionMode) -> bool {
            mode == WorkAttentionMode::Coordinate
        }

        // Wiring the attention item into the graph (parent/related/derived_from
        // edges) is a Coordinate-stance authority.
        helper attention_can_link(mode: WorkAttentionMode) -> bool {
            mode == WorkAttentionMode::Coordinate
        }

        // Which edge kinds an attention-scoped link may create is itself a
        // machine-owned admission verdict over the attention stance, emitted as
        // typed per-kind capability bits. The machine never names domain
        // edge-kind strings; the shell mechanically maps a parsed edge kind to
        // its bit and fails closed for any kind without a bit (today: Blocks and
        // Supersedes carry no bit, so they are denied). Each kind is permitted
        // exactly when an attention-scoped link is reachable at all — i.e. the
        // Coordinate stance that owns graph wiring — preserving the pre-fold
        // fixed allow-list of parent/related/derived_from.
        helper attention_can_link_parent(mode: WorkAttentionMode) -> bool {
            attention_can_link(mode)
        }

        helper attention_can_link_related(mode: WorkAttentionMode) -> bool {
            attention_can_link(mode)
        }

        helper attention_can_link_derived_from(mode: WorkAttentionMode) -> bool {
            attention_can_link(mode)
        }

        helper attention_can_close_own_review_item(
            mode: WorkAttentionMode,
            delegated_authority: AttentionDelegatedAuthority
        ) -> bool {
            (mode == WorkAttentionMode::Review || mode == WorkAttentionMode::Falsify)
                && delegated_authority == AttentionDelegatedAuthority::CloseOwnReviewItem
        }

        helper attention_can_close_if_policy_allows(
            mode: WorkAttentionMode,
            delegated_authority: AttentionDelegatedAuthority
        ) -> bool {
            delegated_authority == AttentionDelegatedAuthority::CloseIfPolicyAllows
                && attention_is_adversarial(mode) == false
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
        disposition AttentionEligibilityClassified => local,
        disposition AttentionAuthorityClassified => local,

        transition PauseActive {
            on input Pause { expected_revision, until_utc_ms }
            guard { self.lifecycle_phase == Phase::Active && self.revision == expected_revision }
            update {
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
                self.paused_until_utc_ms = None;
                self.terminal_at_utc_ms = Some(at_utc_ms);
                self.revision += 1;
            }
            to Stopped
            emit AttentionStopped { revision: self.revision }
        }

        // --- Attention-projection eligibility classification ---
        //
        // The eligibility verdict over the machine-owned lifecycle_phase +
        // paused_until_utc_ms is a machine fact. The shell extracts only the raw
        // wall-clock `now_utc_ms`; the transitions below own the eligibility
        // POLICY and each self-loop in their phase (classification never mutates
        // lifecycle state), emitting exactly one `eligible` verdict. The Paused
        // arms own the deadline-elapsed rule (`paused_until <= now`) the shell
        // used to re-derive.

        transition ClassifyEligibilityActive {
            on input ClassifyAttentionEligibility { now_utc_ms }
            guard "active_is_eligible" { self.lifecycle_phase == Phase::Active }
            update {}
            to Active
            emit AttentionEligibilityClassified { eligible: true }
        }

        transition ClassifyEligibilityPausedElapsed {
            on input ClassifyAttentionEligibility { now_utc_ms }
            guard "paused" { self.lifecycle_phase == Phase::Paused }
            guard "deadline_elapsed" {
                self.paused_until_utc_ms != None
                    && self.paused_until_utc_ms.get("value") <= now_utc_ms
            }
            update {}
            to Paused
            emit AttentionEligibilityClassified { eligible: true }
        }

        transition ClassifyEligibilityPausedPending {
            on input ClassifyAttentionEligibility { now_utc_ms }
            guard "paused" { self.lifecycle_phase == Phase::Paused }
            guard "deadline_not_elapsed" {
                self.paused_until_utc_ms == None
                    || self.paused_until_utc_ms.get("value") > now_utc_ms
            }
            update {}
            to Paused
            emit AttentionEligibilityClassified { eligible: false }
        }

        transition ClassifyEligibilitySuperseded {
            on input ClassifyAttentionEligibility { now_utc_ms }
            guard "superseded_is_ineligible" { self.lifecycle_phase == Phase::Superseded }
            update {}
            to Superseded
            emit AttentionEligibilityClassified { eligible: false }
        }

        transition ClassifyEligibilityStopped {
            on input ClassifyAttentionEligibility { now_utc_ms }
            guard "stopped_is_ineligible" { self.lifecycle_phase == Phase::Stopped }
            update {}
            to Stopped
            emit AttentionEligibilityClassified { eligible: false }
        }

        // --- Projected-authority permission classification ---
        //
        // The per-stance tool-admission capability bits are a machine fact
        // derived purely from the raw binding facts `(mode, delegated_authority)`.
        // This is the complete mode->capability truth table the attention-scoped
        // tool surface mirrors: the shell is a pure tool-name -> capability-bit
        // decoder over these bits, holding no per-mode allow-set of its own. The
        // policy is lifecycle-phase independent, so this self-loops in every
        // phase; it never mutates lifecycle state.
        transition ClassifyAuthority {
            per_phase [Active, Paused, Superseded, Stopped]
            on input ClassifyAttentionAuthority { mode, delegated_authority }
            update {}
            to Active
            emit AttentionAuthorityClassified {
                can_get: attention_can_get(mode),
                can_add_evidence: attention_can_add_evidence(mode),
                can_release: attention_can_release(mode),
                can_update: attention_can_update(mode),
                can_block: attention_can_block(mode),
                can_create: attention_can_create(mode),
                can_link: attention_can_link(mode),
                can_link_parent: attention_can_link_parent(mode),
                can_link_related: attention_can_link_related(mode),
                can_link_derived_from: attention_can_link_derived_from(mode),
                can_close_own_review_item: attention_can_close_own_review_item(mode, delegated_authority),
                can_close_if_policy_allows: attention_can_close_if_policy_allows(mode, delegated_authority)
            }
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
