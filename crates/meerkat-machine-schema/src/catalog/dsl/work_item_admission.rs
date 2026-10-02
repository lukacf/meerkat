//! Exact keyed admission identity of one work item.
//!
//! Separate from `WorkGraphLifecycleMachine` on purpose: the identity is
//! immutable after create and orthogonal to lifecycle, so recording it in the
//! lifecycle machine would double every lifecycle state. The two machines are
//! bound in the `workgraph_attention_bundle` composition: every lifecycle
//! `Created` routes to `Bind` here, and `Bind` only originates from that route.

use super::workgraph_lifecycle::{WorkAdmissionDigestRef, WorkAdmissionKeyRef};
use meerkat_machine_dsl::machine;

/// Machine-owned verdict for a keyed create that found an existing item under
/// the same realm/namespace admission key. The shell extracts the requested
/// key and the owner-computed request digest, drives `ClassifyAdmissionReplay`
/// over the existing item's recovered admission state, and mirrors the
/// verdict: `Replayed` -> return the existing item unchanged, `Conflict` ->
/// typed conflict carrying the existing item id, `KeyMismatch` -> the store
/// index disagrees with machine-owned state; fail closed as a store error.
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
pub enum WorkAdmissionReplayKind {
    #[default]
    KeyMismatch,
    Replayed,
    Conflict,
}

machine! {
    machine WorkItemAdmissionMachine {
        version: 1,
        rust: "self" / "catalog::dsl::work_item_admission",

        state {
            lifecycle_phase: WorkItemAdmissionPhase,
            admission_key: Option<WorkAdmissionKeyRef>,
            request_digest: Option<WorkAdmissionDigestRef>,
        }

        init(Absent) {
            admission_key = None,
            request_digest = None,
        }

        terminal []

        phase WorkItemAdmissionPhase {
            Absent,
            Unkeyed,
            Admitted,
        }

        input WorkItemAdmissionInput {
            // Delivered by the lifecycle `Created` route with that create's
            // identity: both present for a keyed create, both absent otherwise.
            Bind {
                admission_key: Option<WorkAdmissionKeyRef>,
                request_digest: Option<WorkAdmissionDigestRef>,
            },
            // The store returns an existing item when a keyed create finds its
            // realm/namespace admission key already recorded; it never decides
            // whether the replay is exact. The shell drives this input over the
            // EXISTING item's recovered admission state with the requested key
            // and the owner-computed digest of the exact request. A terminal
            // item is still an exact historical replay, never a fresh admission.
            ClassifyAdmissionReplay {
                requested_admission_key: WorkAdmissionKeyRef,
                requested_request_digest: WorkAdmissionDigestRef,
            },
        }

        effect WorkItemAdmissionEffect {
            Bound { keyed: bool },
            AdmissionReplayClassified { admission: Enum<WorkAdmissionReplayKind> },
        }

        invariant admitted_has_identity {
            self.lifecycle_phase != Phase::Admitted
                || (self.admission_key != None && self.request_digest != None)
        }

        invariant non_admitted_has_no_identity {
            self.lifecycle_phase == Phase::Admitted
                || (self.admission_key == None && self.request_digest == None)
        }

        disposition Bound => local seam NoOwnerRealization,
        disposition AdmissionReplayClassified => local seam SurfaceResultAlignment,

        transition BindKeyed {
            on input Bind { admission_key, request_digest }
            guard {
                self.lifecycle_phase == Phase::Absent
                    && admission_key != None
                    && request_digest != None
            }
            update {
                self.admission_key = admission_key;
                self.request_digest = request_digest;
            }
            to Admitted
            emit Bound { keyed: true }
        }

        transition BindUnkeyed {
            on input Bind { admission_key, request_digest }
            guard {
                self.lifecycle_phase == Phase::Absent
                    && admission_key == None
                    && request_digest == None
            }
            update {}
            to Unkeyed
            emit Bound { keyed: false }
        }

        // The request always carries a key and digest (an unkeyed request has
        // nothing to classify), so the three guards are mutually exclusive and
        // total over the recorded identity: the recorded key must equal the
        // requested key for any replay verdict, and only an identical recorded
        // request digest is an exact replay. An unkeyed item is always a key
        // mismatch.

        transition ClassifyAdmissionReplayExact {
            per_phase [Unkeyed, Admitted]
            on input ClassifyAdmissionReplay { requested_admission_key, requested_request_digest }
            guard "admission_replay_exact" {
                self.admission_key == Some(requested_admission_key)
                    && self.request_digest == Some(requested_request_digest)
            }
            update {}
            to Absent
            emit AdmissionReplayClassified { admission: WorkAdmissionReplayKind::Replayed }
        }

        transition ClassifyAdmissionReplayConflict {
            per_phase [Unkeyed, Admitted]
            on input ClassifyAdmissionReplay { requested_admission_key, requested_request_digest }
            guard "admission_replay_conflict" {
                self.admission_key == Some(requested_admission_key)
                    && self.request_digest != Some(requested_request_digest)
            }
            update {}
            to Absent
            emit AdmissionReplayClassified { admission: WorkAdmissionReplayKind::Conflict }
        }

        transition ClassifyAdmissionReplayKeyMismatch {
            per_phase [Unkeyed, Admitted]
            on input ClassifyAdmissionReplay { requested_admission_key, requested_request_digest }
            guard "admission_replay_key_mismatch" {
                self.admission_key != Some(requested_admission_key)
            }
            update {}
            to Absent
            emit AdmissionReplayClassified { admission: WorkAdmissionReplayKind::KeyMismatch }
        }
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
struct WorkItemAdmissionMachineStateWire {
    lifecycle_phase: WorkItemAdmissionPhase,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    admission_key: Option<WorkAdmissionKeyRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    request_digest: Option<WorkAdmissionDigestRef>,
}

impl serde::Serialize for WorkItemAdmissionMachineState {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        WorkItemAdmissionMachineStateWire {
            lifecycle_phase: self.lifecycle_phase,
            admission_key: self.admission_key.clone(),
            request_digest: self.request_digest.clone(),
        }
        .serialize(serializer)
    }
}

impl<'de> serde::Deserialize<'de> for WorkItemAdmissionMachineState {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        WorkItemAdmissionMachineStateWire::deserialize(deserializer).map(|wire| Self {
            lifecycle_phase: wire.lifecycle_phase,
            admission_key: wire.admission_key,
            request_digest: wire.request_digest,
        })
    }
}

impl serde::Serialize for WorkItemAdmissionPhase {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(match self {
            Self::Absent => "absent",
            Self::Unkeyed => "unkeyed",
            Self::Admitted => "admitted",
        })
    }
}

impl<'de> serde::Deserialize<'de> for WorkItemAdmissionPhase {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = <String as serde::Deserialize>::deserialize(deserializer)?;
        match value.as_str() {
            "absent" => Ok(Self::Absent),
            "unkeyed" => Ok(Self::Unkeyed),
            "admitted" => Ok(Self::Admitted),
            other => Err(serde::de::Error::custom(format!(
                "invalid WorkItemAdmissionPhase `{other}`"
            ))),
        }
    }
}
