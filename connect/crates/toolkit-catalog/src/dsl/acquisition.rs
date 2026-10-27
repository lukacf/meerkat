//! Canonical per-request acquisition lifecycle.
//!
//! Strings here are exact identity, digest, or immutable evidence references.
//! Full domain values and verification of readiness belong to the production
//! bridge. Generated inputs are trusted host commands, not model tool inputs.

// The upstream macro materializes String literals for typed DSL expressions.
// Keep this generated-code lint allowance local to the canonical DSL module.
#![allow(clippy::cmp_owned)]

use meerkat_machine_dsl::machine;

machine! {
    machine ToolkitAcquisitionMachine {
        version: 1,
        rust: "meerkat_toolkit" / "setup",

        state {
            lifecycle_phase: AcquisitionPhase,
            operation_id: String,
            request_digest: String,
            authority_digest: String,
            continuation_id: String,
            deadline: u64,
            admitted_at: u64,
            last_observed_at: u64,
            revision: u64,
            selection_digest: String,
            decision_ref: String,
            selection_generation: u64,
            readiness_ref: String,
            readiness_digest: String,
            readiness_observed_at: u64,
            detail_ref: String,
            terminal_revision: u64,
            terminal_at: u64,
            continuation_receipt_ref: String,
        }

        init(Absent) {
            operation_id = "",
            request_digest = "",
            authority_digest = "",
            continuation_id = "",
            deadline = 0,
            admitted_at = 0,
            last_observed_at = 0,
            revision = 0,
            selection_digest = "",
            decision_ref = "",
            selection_generation = 0,
            readiness_ref = "",
            readiness_digest = "",
            readiness_observed_at = 0,
            detail_ref = "",
            terminal_revision = 0,
            terminal_at = 0,
            continuation_receipt_ref = "",
        }

        terminal [Ready, Unavailable, Failed, Cancelled, Expired]

        phase AcquisitionPhase {
            Absent,
            Investigating,
            Preparing,
            Ready,
            Unavailable,
            Failed,
            Cancelled,
            Expired,
        }

        input AcquisitionInput {
            Admit {
                operation_id: String,
                request_digest: String,
                authority_digest: String,
                continuation_id: String,
                expected_revision: u64,
                deadline: u64,
                now: u64,
            },
            RecordSelection {
                operation_id: String,
                authority_digest: String,
                expected_revision: u64,
                selection_digest: String,
                decision_ref: String,
                now: u64,
            },
            ResolveReady {
                operation_id: String,
                authority_digest: String,
                expected_revision: u64,
                selection_digest: String,
                selection_generation: u64,
                readiness_ref: String,
                readiness_digest: String,
                observed_at: u64,
                now: u64,
            },
            ReportUnavailable {
                operation_id: String,
                authority_digest: String,
                expected_revision: u64,
                detail_ref: String,
                now: u64,
            },
            ReportFailure {
                operation_id: String,
                authority_digest: String,
                expected_revision: u64,
                detail_ref: String,
                now: u64,
            },
            Cancel {
                operation_id: String,
                authority_digest: String,
                expected_revision: u64,
                detail_ref: String,
                now: u64,
            },
            Expire {
                operation_id: String,
                authority_digest: String,
                expected_revision: u64,
                now: u64,
            },
            AcknowledgeContinuation {
                operation_id: String,
                authority_digest: String,
                expected_revision: u64,
                continuation_id: String,
                terminal_revision: u64,
                receipt_ref: String,
            },
        }

        effect AcquisitionEffect {
            WorkAdmissionRequested {
                operation_id: String,
                request_digest: String,
                authority_digest: String,
            },
            SelectionRecorded {
                operation_id: String,
                selection_digest: String,
                selection_generation: u64,
                decision_ref: String,
            },
            ContinuationRequested {
                operation_id: String,
                continuation_id: String,
                terminal_revision: u64,
                outcome: AcquisitionPhase,
            },
            ContinuationAcknowledged {
                operation_id: String,
                continuation_id: String,
                terminal_revision: u64,
                receipt_ref: String,
            },
        }

        disposition WorkAdmissionRequested => local seam OwnerRealizationOnly,
        disposition SelectionRecorded => local seam SurfaceResultAlignment,
        disposition ContinuationRequested => local seam OwnerRealizationPlusFeedback,
        disposition ContinuationAcknowledged => local seam SurfaceResultAlignment,

        helper active(phase: AcquisitionPhase) -> bool {
            phase == Phase::Investigating || phase == Phase::Preparing
        }

        helper terminal(phase: AcquisitionPhase) -> bool {
            phase == Phase::Ready || phase == Phase::Unavailable
                || phase == Phase::Failed || phase == Phase::Cancelled
                || phase == Phase::Expired
        }

        invariant absent_is_empty {
            self.lifecycle_phase != Phase::Absent
                || (self.operation_id == "" && self.request_digest == ""
                    && self.authority_digest == "" && self.continuation_id == ""
                    && self.deadline == 0 && self.admitted_at == 0
                    && self.last_observed_at == 0 && self.revision == 0
                    && self.selection_digest == "" && self.decision_ref == ""
                    && self.selection_generation == 0)
        }

        invariant admitted_identity_is_bound {
            self.lifecycle_phase == Phase::Absent
                || (self.operation_id != "" && self.request_digest != ""
                    && self.authority_digest != "" && self.continuation_id != ""
                    && self.deadline > self.admitted_at
                    && self.last_observed_at >= self.admitted_at && self.revision > 0)
        }

        invariant selection_has_exact_evidence {
            (self.selection_generation == 0 && self.selection_digest == ""
                && self.decision_ref == "")
                || (self.selection_generation > 0 && self.selection_digest != ""
                    && self.decision_ref != "" && self.selection_generation < self.revision)
        }

        invariant investigating_has_no_selection {
            self.lifecycle_phase != Phase::Investigating
                || (self.selection_generation == 0 && self.revision == 1)
        }

        invariant preparing_has_selection {
            self.lifecycle_phase != Phase::Preparing || self.selection_generation > 0
        }

        invariant active_precedes_deadline {
            !active(self.lifecycle_phase) || self.last_observed_at < self.deadline
        }

        invariant revision_tracks_selection_and_resolution {
            (!active(self.lifecycle_phase)
                || (self.selection_generation < u64::MAX - 2
                    && self.revision == self.selection_generation + 1
                    && self.revision <= u64::MAX - 2))
                && (!terminal(self.lifecycle_phase)
                    || (self.selection_generation < u64::MAX - 2
                        && self.terminal_revision == self.selection_generation + 2
                        && self.terminal_revision < u64::MAX))
        }

        invariant non_expiry_resolution_precedes_deadline {
            (self.lifecycle_phase != Phase::Ready && self.lifecycle_phase != Phase::Unavailable
                && self.lifecycle_phase != Phase::Failed) || self.terminal_at < self.deadline
        }

        invariant ready_has_current_observation {
            self.lifecycle_phase != Phase::Ready
                || (self.selection_generation > 0 && self.readiness_ref != ""
                    && self.readiness_digest != ""
                    && self.readiness_observed_at == self.terminal_at
                    && self.readiness_observed_at == self.last_observed_at
                    && self.readiness_observed_at < self.deadline)
        }

        invariant other_outcomes_have_no_readiness {
            self.lifecycle_phase == Phase::Ready
                || (self.readiness_ref == "" && self.readiness_digest == ""
                    && self.readiness_observed_at == 0)
        }

        invariant terminal_has_one_continuation {
            !terminal(self.lifecycle_phase)
                || (self.terminal_revision > 0 && self.terminal_revision <= self.revision
                    && self.terminal_at == self.last_observed_at
                    && self.terminal_at >= self.admitted_at)
        }

        invariant nonterminal_has_no_continuation {
            terminal(self.lifecycle_phase)
                || (self.terminal_revision == 0 && self.terminal_at == 0
                    && self.continuation_receipt_ref == "")
        }

        invariant pending_continuation_retains_terminal_revision {
            !terminal(self.lifecycle_phase) || self.continuation_receipt_ref != ""
                || self.revision == self.terminal_revision
        }

        invariant acknowledged_continuation_advances_once {
            self.continuation_receipt_ref == ""
                || (terminal(self.lifecycle_phase) && self.terminal_revision < u64::MAX
                    && self.revision == self.terminal_revision + 1)
        }

        invariant expiry_observes_deadline {
            self.lifecycle_phase != Phase::Expired || self.terminal_at >= self.deadline
        }

        invariant resolution_detail_matches_outcome {
            ((self.lifecycle_phase == Phase::Unavailable || self.lifecycle_phase == Phase::Failed
                || self.lifecycle_phase == Phase::Cancelled) && self.detail_ref != "")
                || ((self.lifecycle_phase != Phase::Unavailable && self.lifecycle_phase != Phase::Failed
                    && self.lifecycle_phase != Phase::Cancelled) && self.detail_ref == "")
        }

        transition AdmitRequest {
            on input Admit { operation_id, request_digest, authority_digest, continuation_id, expected_revision, deadline, now }
            guard {
                self.lifecycle_phase == Phase::Absent && expected_revision == self.revision
                    && operation_id != "" && request_digest != ""
                    && authority_digest != "" && continuation_id != "" && deadline > now
            }
            update {
                self.operation_id = operation_id;
                self.request_digest = request_digest;
                self.authority_digest = authority_digest;
                self.continuation_id = continuation_id;
                self.deadline = deadline;
                self.admitted_at = now;
                self.last_observed_at = now;
                self.revision += 1;
            }
            to Investigating
            emit WorkAdmissionRequested { operation_id: self.operation_id, request_digest: self.request_digest, authority_digest: self.authority_digest }
        }

        transition SelectImplementation {
            on input RecordSelection { operation_id, authority_digest, expected_revision, selection_digest, decision_ref, now }
            guard {
                active(self.lifecycle_phase) && operation_id == self.operation_id
                    && authority_digest == self.authority_digest && expected_revision == self.revision
                    && self.revision < u64::MAX - 2 && self.selection_generation < u64::MAX - 2
                    && selection_digest != "" && selection_digest != self.selection_digest
                    && decision_ref != "" && now >= self.last_observed_at && now < self.deadline
            }
            update {
                self.selection_digest = selection_digest;
                self.decision_ref = decision_ref;
                self.selection_generation += 1;
                self.readiness_ref = "";
                self.readiness_digest = "";
                self.readiness_observed_at = 0;
                self.last_observed_at = now;
                self.revision += 1;
            }
            to Preparing
            emit SelectionRecorded { operation_id: self.operation_id, selection_digest: self.selection_digest, selection_generation: self.selection_generation, decision_ref: self.decision_ref }
        }

        transition ResolveCurrentReadiness {
            on input ResolveReady { operation_id, authority_digest, expected_revision, selection_digest, selection_generation, readiness_ref, readiness_digest, observed_at, now }
            guard {
                self.lifecycle_phase == Phase::Preparing && operation_id == self.operation_id
                    && authority_digest == self.authority_digest && expected_revision == self.revision
                    && self.revision < u64::MAX - 1 && selection_digest == self.selection_digest
                    && selection_generation == self.selection_generation
                    && readiness_ref != "" && readiness_digest != "" && observed_at == now
                    && now >= self.last_observed_at && now < self.deadline
            }
            update {
                self.readiness_ref = readiness_ref;
                self.readiness_digest = readiness_digest;
                self.readiness_observed_at = observed_at;
                self.last_observed_at = now;
                self.revision += 1;
                self.terminal_revision = self.revision;
                self.terminal_at = now;
            }
            to Ready
            emit ContinuationRequested { operation_id: self.operation_id, continuation_id: self.continuation_id, terminal_revision: self.terminal_revision, outcome: Phase::Ready }
        }

        transition ResolveUnavailable {
            on input ReportUnavailable { operation_id, authority_digest, expected_revision, detail_ref, now }
            guard {
                active(self.lifecycle_phase) && operation_id == self.operation_id
                    && authority_digest == self.authority_digest && expected_revision == self.revision
                    && self.revision < u64::MAX - 1 && detail_ref != ""
                    && now >= self.last_observed_at && now < self.deadline
            }
            update {
                self.detail_ref = detail_ref;
                self.last_observed_at = now;
                self.revision += 1;
                self.terminal_revision = self.revision;
                self.terminal_at = now;
            }
            to Unavailable
            emit ContinuationRequested { operation_id: self.operation_id, continuation_id: self.continuation_id, terminal_revision: self.terminal_revision, outcome: Phase::Unavailable }
        }

        transition ResolveFailure {
            on input ReportFailure { operation_id, authority_digest, expected_revision, detail_ref, now }
            guard {
                active(self.lifecycle_phase) && operation_id == self.operation_id
                    && authority_digest == self.authority_digest && expected_revision == self.revision
                    && self.revision < u64::MAX - 1 && detail_ref != ""
                    && now >= self.last_observed_at && now < self.deadline
            }
            update {
                self.detail_ref = detail_ref;
                self.last_observed_at = now;
                self.revision += 1;
                self.terminal_revision = self.revision;
                self.terminal_at = now;
            }
            to Failed
            emit ContinuationRequested { operation_id: self.operation_id, continuation_id: self.continuation_id, terminal_revision: self.terminal_revision, outcome: Phase::Failed }
        }

        transition CancelRequest {
            on input Cancel { operation_id, authority_digest, expected_revision, detail_ref, now }
            guard {
                active(self.lifecycle_phase) && operation_id == self.operation_id
                    && authority_digest == self.authority_digest && expected_revision == self.revision
                    && self.revision < u64::MAX - 1 && detail_ref != "" && now >= self.last_observed_at
            }
            update {
                self.detail_ref = detail_ref;
                self.last_observed_at = now;
                self.revision += 1;
                self.terminal_revision = self.revision;
                self.terminal_at = now;
            }
            to Cancelled
            emit ContinuationRequested { operation_id: self.operation_id, continuation_id: self.continuation_id, terminal_revision: self.terminal_revision, outcome: Phase::Cancelled }
        }

        transition ExpireRequest {
            on input Expire { operation_id, authority_digest, expected_revision, now }
            guard {
                active(self.lifecycle_phase) && operation_id == self.operation_id
                    && authority_digest == self.authority_digest && expected_revision == self.revision
                    && self.revision < u64::MAX - 1 && now >= self.last_observed_at && now >= self.deadline
            }
            update {
                self.last_observed_at = now;
                self.revision += 1;
                self.terminal_revision = self.revision;
                self.terminal_at = now;
            }
            to Expired
            emit ContinuationRequested { operation_id: self.operation_id, continuation_id: self.continuation_id, terminal_revision: self.terminal_revision, outcome: Phase::Expired }
        }

        transition AcknowledgeContinuation {
            per_phase [Ready, Unavailable, Failed, Cancelled, Expired]
            on input AcknowledgeContinuation { operation_id, authority_digest, expected_revision, continuation_id, terminal_revision, receipt_ref }
            guard {
                operation_id == self.operation_id && authority_digest == self.authority_digest
                    && expected_revision == self.revision && self.revision < u64::MAX
                    && continuation_id == self.continuation_id && terminal_revision == self.terminal_revision
                    && receipt_ref != "" && self.continuation_receipt_ref == ""
            }
            update {
                self.continuation_receipt_ref = receipt_ref;
                self.revision += 1;
            }
            to Ready
            emit ContinuationAcknowledged { operation_id: self.operation_id, continuation_id: self.continuation_id, terminal_revision: self.terminal_revision, receipt_ref: self.continuation_receipt_ref }
        }

        transition ObserveSameContinuationAcknowledgment {
            per_phase [Ready, Unavailable, Failed, Cancelled, Expired]
            on input AcknowledgeContinuation { operation_id, authority_digest, expected_revision, continuation_id, terminal_revision, receipt_ref }
            guard {
                operation_id == self.operation_id && authority_digest == self.authority_digest
                    && expected_revision == self.revision && continuation_id == self.continuation_id
                    && terminal_revision == self.terminal_revision && receipt_ref != ""
                    && receipt_ref == self.continuation_receipt_ref
            }
            update {}
            to Ready
        }
    }
}
