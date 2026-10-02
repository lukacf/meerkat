# WorkItemAdmissionMachine

_Generated from the Rust machine catalog. Do not edit by hand._

- Version: `1`
- Rust owner: `self` / `catalog::dsl::work_item_admission`

## State
- Phase enum: `Absent | Unkeyed | Admitted`
- `admission_key`: `Option<WorkAdmissionKeyRef>`
- `request_digest`: `Option<WorkAdmissionDigestRef>`

## Inputs
- `Bind`(admission_key: Option<WorkAdmissionKeyRef>, request_digest: Option<WorkAdmissionDigestRef>)
- `ClassifyAdmissionReplay`(requested_admission_key: WorkAdmissionKeyRef, requested_request_digest: WorkAdmissionDigestRef)

## Signals

## Effects
- `Bound`(keyed: Bool)
- `AdmissionReplayClassified`(admission: WorkAdmissionReplayKind)

## Invariants
- `admitted_has_identity`
- `non_admitted_has_no_identity`

## Transitions
### `BindKeyed`
- From: `Absent`
- On: `Bind`(admission_key, request_digest)
- Guards:
  - ``
- Emits: `Bound`
- To: `Admitted`

### `BindUnkeyed`
- From: `Absent`
- On: `Bind`(admission_key, request_digest)
- Guards:
  - ``
- Emits: `Bound`
- To: `Unkeyed`

### `ClassifyAdmissionReplayExactUnkeyed`
- From: `Unkeyed`
- On: `ClassifyAdmissionReplay`(requested_admission_key, requested_request_digest)
- Guards:
  - `admission_replay_exact`
- Emits: `AdmissionReplayClassified`
- To: `Unkeyed`

### `ClassifyAdmissionReplayExactAdmitted`
- From: `Admitted`
- On: `ClassifyAdmissionReplay`(requested_admission_key, requested_request_digest)
- Guards:
  - `admission_replay_exact`
- Emits: `AdmissionReplayClassified`
- To: `Admitted`

### `ClassifyAdmissionReplayConflictUnkeyed`
- From: `Unkeyed`
- On: `ClassifyAdmissionReplay`(requested_admission_key, requested_request_digest)
- Guards:
  - `admission_replay_conflict`
- Emits: `AdmissionReplayClassified`
- To: `Unkeyed`

### `ClassifyAdmissionReplayConflictAdmitted`
- From: `Admitted`
- On: `ClassifyAdmissionReplay`(requested_admission_key, requested_request_digest)
- Guards:
  - `admission_replay_conflict`
- Emits: `AdmissionReplayClassified`
- To: `Admitted`

### `ClassifyAdmissionReplayKeyMismatchAbsent`
- From: `Absent`
- On: `ClassifyAdmissionReplay`(requested_admission_key, requested_request_digest)
- Guards:
  - `admission_replay_key_mismatch`
- Emits: `AdmissionReplayClassified`
- To: `Absent`

### `ClassifyAdmissionReplayKeyMismatchUnkeyed`
- From: `Unkeyed`
- On: `ClassifyAdmissionReplay`(requested_admission_key, requested_request_digest)
- Guards:
  - `admission_replay_key_mismatch`
- Emits: `AdmissionReplayClassified`
- To: `Unkeyed`

### `ClassifyAdmissionReplayKeyMismatchAdmitted`
- From: `Admitted`
- On: `ClassifyAdmissionReplay`(requested_admission_key, requested_request_digest)
- Guards:
  - `admission_replay_key_mismatch`
- Emits: `AdmissionReplayClassified`
- To: `Admitted`

## Coverage
### Code Anchors
- `work_item_admission` (machine `WorkItemAdmissionMachine`): `crates/meerkat-workgraph/src/machine.rs` — WorkItemAdmissionMachine owner of a work item's exact keyed admission identity: BindKeyed and BindUnkeyed record (or decline) the identity delivered by the lifecycle Created route, and ClassifyAdmissionReplayExact, ClassifyAdmissionReplayConflict and ClassifyAdmissionReplayKeyMismatch decide, over the recovered identity, whether a keyed create that found an existing item is an exact replay, a typed conflict, or a store-index mismatch; effects Bound, AdmissionReplayClassified; invariants admitted_has_identity, non_admitted_has_no_identity

### Scenarios
- `work_item_admission_replay` — a keyed create binds its identity once; an exact replay under the same key and digest is Replayed, the same key with another digest is Conflict, and another key (or an unkeyed or never-bound item) is KeyMismatch, in every phase
