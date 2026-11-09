//! Source eligibility and cancellation authority, without native admission facts.
//!
//! References identify immutable, validated bridge records. Prepared transitions
//! and their effects must commit together before the bridge realizes any effect.
//! Cancellation closes future eligibility; it does not settle an existing offer.
#![allow(clippy::cmp_owned)]

use super::original_types::{ToolkitSourceKind, ToolkitSourceReadiness};
use meerkat_machine_dsl::machine;

machine! {
    machine ToolkitSourceRecordMachine {
        version: 1,
        rust: "toolkit_catalog" / "source_record",
        state {
            source_phase: ToolkitSourcePhase,
            source_key: String,
            scope_digest: String,
            source_kind: Enum<ToolkitSourceKind>,
            helper_parent_report_ref: String,
            readiness: Enum<ToolkitSourceReadiness>,
            preparation_id: String,
            preparation_receipt: String,
            cancellation_id: String,
            revocation_receipt: String,
            publication_receipt: String,
            revision: u64,
        }
        init(Absent) {
            source_key = "",
            scope_digest = "",
            source_kind = ToolkitSourceKind::Ordinary,
            helper_parent_report_ref = "",
            readiness = ToolkitSourceReadiness::ClosedToOffers,
            preparation_id = "",
            preparation_receipt = "",
            cancellation_id = "",
            revocation_receipt = "",
            publication_receipt = "",
            revision = 0,
        }
        terminal [Cancelled]
        phase ToolkitSourcePhase { Absent, Open, CancellationPending, Cancelled }
        input ToolkitSourceInput {
            Register {
                source_key: String, scope_digest: String,
                source_kind: Enum<ToolkitSourceKind>, helper_parent_report_ref: String,
                preparation_id: String,
            },
            ConfirmPreparation {
                source_key: String, expected_revision: u64,
                preparation_id: String, preparation_receipt: String,
            },
            BeginCancellation {
                source_key: String, expected_revision: u64, cancellation_id: String,
            },
            ConfirmCancellation {
                source_key: String, expected_revision: u64, cancellation_id: String,
                published_revision: u64, revocation_receipt: String,
                publication_receipt: String,
            },
        }
        effect ToolkitSourceEffect {
            PrepareSource { source_key: String, preparation_id: String },
            RevokeSource { source_key: String, cancellation_id: String },
            PublishSnapshot { source_key: String, revision: u64 },
        }
        disposition PrepareSource => local seam NoOwnerRealization,
        disposition RevokeSource => local seam NoOwnerRealization,
        disposition PublishSnapshot => local seam SurfaceResultAlignment,

        invariant absent_source_has_no_uncommitted_facts {
            self.source_phase != Phase::Absent ||
                (self.source_key == "" && self.scope_digest == "" && self.source_kind == ToolkitSourceKind::Ordinary
                && self.helper_parent_report_ref == "" && self.readiness == ToolkitSourceReadiness::ClosedToOffers
                && self.preparation_id == "" && self.preparation_receipt == "" && self.cancellation_id == ""
                && self.revocation_receipt == "" && self.publication_receipt == "" && self.revision == 0)
        }
        invariant identity_is_complete {
            self.source_phase == Phase::Absent ||
                (self.source_key != "" && self.scope_digest != "" && self.preparation_id != "" && self.revision > 0)
        }
        invariant helper_identity_is_explicit {
            self.source_kind != ToolkitSourceKind::Helper || self.helper_parent_report_ref != ""
        }
        invariant ordinary_source_has_no_helper_identity {
            self.source_kind != ToolkitSourceKind::Ordinary || self.helper_parent_report_ref == ""
        }
        invariant open_source_has_typed_readiness {
            self.source_phase != Phase::Open ||
                (self.readiness == ToolkitSourceReadiness::Preparing || self.readiness == ToolkitSourceReadiness::Ready)
        }
        invariant preparation_is_not_a_completed_effect {
            self.readiness != ToolkitSourceReadiness::Preparing ||
                (self.source_phase == Phase::Open && self.preparation_receipt == "")
        }
        invariant readiness_requires_preparation {
            self.readiness != ToolkitSourceReadiness::Ready ||
                (self.source_phase == Phase::Open && self.preparation_receipt != "")
        }
        invariant cancellation_closes_eligibility {
            (self.source_phase != Phase::CancellationPending && self.source_phase != Phase::Cancelled) ||
                (self.readiness == ToolkitSourceReadiness::ClosedToOffers && self.cancellation_id != "")
        }
        invariant cancellation_requires_observed_effects {
            self.source_phase != Phase::Cancelled ||
                (self.revocation_receipt != "" && self.publication_receipt != "")
        }
        invariant cancellation_receipts_require_completion {
            self.source_phase == Phase::Cancelled ||
                (self.revocation_receipt == "" && self.publication_receipt == "")
        }

        transition RegisterSource {
            on input Register { source_key, scope_digest, source_kind, helper_parent_report_ref, preparation_id }
            guard {
                self.source_phase == Phase::Absent && source_key != "" && scope_digest != "" && preparation_id != ""
                && (source_kind != ToolkitSourceKind::Helper || helper_parent_report_ref != "")
                && (source_kind != ToolkitSourceKind::Ordinary || helper_parent_report_ref == "")
            }
            update {
                self.source_key = source_key; self.scope_digest = scope_digest;
                self.source_kind = source_kind; self.helper_parent_report_ref = helper_parent_report_ref;
                self.preparation_id = preparation_id;
                self.readiness = ToolkitSourceReadiness::Preparing; self.revision += 1;
            }
            to Open
            emit PrepareSource { source_key: self.source_key, preparation_id: self.preparation_id }
            emit PublishSnapshot { source_key: self.source_key, revision: self.revision }
        }
        transition MakeSourceReady {
            on input ConfirmPreparation { source_key, expected_revision, preparation_id, preparation_receipt }
            guard {
                self.source_phase == Phase::Open && self.readiness == ToolkitSourceReadiness::Preparing
                && source_key == self.source_key && expected_revision == self.revision
                && preparation_id == self.preparation_id && preparation_receipt != ""
                && self.revision < u64::MAX - 2
            }
            update {
                self.preparation_receipt = preparation_receipt;
                self.readiness = ToolkitSourceReadiness::Ready; self.revision += 1;
            }
            to Open
            emit PublishSnapshot { source_key: self.source_key, revision: self.revision }
        }
        transition CloseSourceEligibility {
            on input BeginCancellation { source_key, expected_revision, cancellation_id }
            guard {
                self.source_phase == Phase::Open && source_key == self.source_key
                && expected_revision == self.revision && cancellation_id != "" && self.revision < u64::MAX - 1
            }
            update {
                self.cancellation_id = cancellation_id;
                self.readiness = ToolkitSourceReadiness::ClosedToOffers; self.revision += 1;
            }
            to CancellationPending
            emit RevokeSource { source_key: self.source_key, cancellation_id: self.cancellation_id }
            emit PublishSnapshot { source_key: self.source_key, revision: self.revision }
        }
        transition ConfirmSourceCancellation {
            on input ConfirmCancellation {
                source_key, expected_revision, cancellation_id, published_revision,
                revocation_receipt, publication_receipt
            }
            guard {
                self.source_phase == Phase::CancellationPending && source_key == self.source_key
                && expected_revision == self.revision && cancellation_id == self.cancellation_id
                && published_revision == self.revision && revocation_receipt != "" && publication_receipt != ""
                && self.revision < u64::MAX
            }
            update {
                self.revocation_receipt = revocation_receipt;
                self.publication_receipt = publication_receipt; self.revision += 1;
            }
            to Cancelled
            emit PublishSnapshot { source_key: self.source_key, revision: self.revision }
        }
    }
}
