//! Canonical native owner of bounded Toolkit component lifetime.
//! Ordinary paired host callback token/activity custody still requires migration.
//!
//! Admission closure is separate from physical settlement. Only the production
//! bridge can observe custody locks and submit ConfirmSettlement; zero activity
//! counters or a lost process endpoint cannot supply that observation.
#![allow(clippy::cmp_owned)]
use meerkat_machine_dsl::machine;

machine! {
    machine ToolkitComponentLifetimeMachine {
        version: 1,
        rust: "meerkat_toolkit" / "host_lifetime",
        state {
            lifetime_phase: ComponentLifetimePhase,
            incarnation: String,
            component: String,
            activation_digest: String,
            revision: u64,
            close_id: String,
            close_reason: String,
            quarantined: bool,
        }
        init(Absent) {
            incarnation = "",
            component = "",
            activation_digest = "",
            revision = 0,
            close_id = "",
            close_reason = "",
            quarantined = false,
        }
        terminal [Closed, Quarantined, Uncertain, Failed]
        phase ComponentLifetimePhase {
            Absent,
            Staged,
            Open,
            Closing,
            Closed,
            Quarantined,
            Uncertain,
            Failed,
        }
        input ComponentLifetimeInput {
            Stage { incarnation: String, component: String, activation_digest: String },
            Open { incarnation: String, expected_revision: u64 },
            BeginClose { incarnation: String, expected_revision: u64, close_id: String, reason: String, quarantine: bool },
            ConfirmSettlement { incarnation: String, expected_revision: u64, close_id: String },
            ObserveUncertainty { incarnation: String, expected_revision: u64, close_id: String },
            ObserveFailure { incarnation: String, expected_revision: u64, close_id: String },
        }
        effect ComponentLifetimeEffect {
            Changed { revision: u64 },
        }
        disposition Changed => local seam SurfaceResultAlignment,
        invariant absent_has_no_identity {
            self.lifetime_phase != Phase::Absent || (self.incarnation == "" && self.component == "" && self.activation_digest == "" && self.revision == 0)
        }
        invariant staged_has_identity {
            self.lifetime_phase == Phase::Absent || (self.incarnation != "" && self.component != "" && self.activation_digest != "" && self.revision > 0)
        }
        invariant closing_has_correlation {
            self.lifetime_phase == Phase::Absent || self.lifetime_phase == Phase::Staged || self.lifetime_phase == Phase::Open || (self.close_id != "" && self.close_reason != "")
        }
        invariant terminal_closure_never_open {
            self.lifetime_phase != Phase::Quarantined || self.quarantined
        }
        transition StageComponent {
            on input Stage { incarnation, component, activation_digest }
            guard { self.lifetime_phase == Phase::Absent && incarnation != "" && component != "" && activation_digest != "" }
            update { self.incarnation = incarnation; self.component = component; self.activation_digest = activation_digest; self.revision += 1; }
            to Staged
            emit Changed { revision: self.revision }
        }
        transition OpenComponent {
            on input Open { incarnation, expected_revision }
            guard { self.lifetime_phase == Phase::Staged && incarnation == self.incarnation && expected_revision == self.revision && self.revision < u64::MAX - 2 }
            update { self.revision += 1; }
            to Open
            emit Changed { revision: self.revision }
        }
        transition CloseAdmission {
            on input BeginClose { incarnation, expected_revision, close_id, reason, quarantine }
            guard { (self.lifetime_phase == Phase::Staged || self.lifetime_phase == Phase::Open) && incarnation == self.incarnation && expected_revision == self.revision && self.revision < u64::MAX - 1 && close_id != "" && reason != "" }
            update { self.close_id = close_id; self.close_reason = reason; self.quarantined = quarantine; self.revision += 1; }
            to Closing
            emit Changed { revision: self.revision }
        }
        transition SettleClosed {
            on input ConfirmSettlement { incarnation, expected_revision, close_id }
            guard { self.lifetime_phase == Phase::Closing && !self.quarantined && incarnation == self.incarnation && expected_revision == self.revision && close_id == self.close_id }
            update { self.revision += 1; }
            to Closed
            emit Changed { revision: self.revision }
        }
        transition SettleQuarantine {
            on input ConfirmSettlement { incarnation, expected_revision, close_id }
            guard { self.lifetime_phase == Phase::Closing && self.quarantined && incarnation == self.incarnation && expected_revision == self.revision && close_id == self.close_id }
            update { self.revision += 1; }
            to Quarantined
            emit Changed { revision: self.revision }
        }
        transition PreserveUncertainty {
            on input ObserveUncertainty { incarnation, expected_revision, close_id }
            guard { self.lifetime_phase == Phase::Closing && incarnation == self.incarnation && expected_revision == self.revision && close_id == self.close_id }
            update { self.revision += 1; }
            to Uncertain
            emit Changed { revision: self.revision }
        }
        transition PreserveFailure {
            on input ObserveFailure { incarnation, expected_revision, close_id }
            guard { self.lifetime_phase == Phase::Closing && incarnation == self.incarnation && expected_revision == self.revision && close_id == self.close_id }
            update { self.revision += 1; }
            to Failed
            emit Changed { revision: self.revision }
        }
    }
}
