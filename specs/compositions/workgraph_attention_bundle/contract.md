# workgraph_attention_bundle

_Generated from the Rust composition catalog. Do not edit by hand._

## Machines
- `workgraph`: `WorkGraphLifecycleMachine` @ actor `workgraph_authority`
- `attention`: `WorkAttentionLifecycleMachine` @ actor `attention_authority`
- `admission`: `WorkItemAdmissionMachine` @ actor `admission_authority`

## Routes
- `work_item_create_binds_admission`: `workgraph`.`Created` -> `admission`.`Bind` [Immediate]
- `work_item_close_stops_attention`: `workgraph`.`Closed` -> `attention`.`Stop` [Immediate]

## Target Selectors
- `(none)`

## Driver
- `(none)`

## Transaction Plans
- `transactional_create_binds_admission` via `create_work_item` / `WorkGraphStore::insert_item_admitted` — a work item create and its admission identity commit together: the item row, its Created event and (for a keyed create) the realm/namespace key index are written in one store transaction, so no item exists without its identity and no identity without its item
- `transactional_close_stops_attention` via `close_work_item` / `WorkGraphStore::update_item_and_attention_cas` — terminal work item close atomically stops one co-resident live attention binding; production fan-out applies this transaction per binding

## Scheduler Rules
- `(none)`

## Structural Requirements
- `work_item_create_routes_to_admission_bind` — every work item create obligates the admission bind for that create's identity, so a keyed item never exists without its Admitted identity
- `closed_work_item_routes_to_attention_stop` — terminal WorkGraph item closure stops co-resident attention bindings through the canonical WorkGraph-to-attention route

## Behavioral Invariants
- `admission_bind_originates_from_work_item_create` — an admission identity is bound only by a work item create, so there is no orphan admission
- `attention_stop_originates_from_work_item_close` — attention stop on terminal item closure is not ad hoc service-only mutation; it originates from the WorkGraph Closed effect route

## Coverage
### Code Anchors
- `workgraph_attention_service_close` (route `work_item_close_stops_attention`): `crates/meerkat-workgraph/src/service.rs` — WorkGraph service close path realizes the canonical WorkGraph Closed to WorkAttention Stop route with an atomic item-and-attention CAS update
- `workgraph_attention_bundle_schema` (route `work_item_close_stops_attention`): `crates/meerkat-machine-schema/src/catalog/compositions.rs` — formal WorkGraph item closure to WorkAttention stop composition

### Scenarios
- `close-stops-attention` — terminal WorkGraph item closure routes to WorkAttention Stop so live goal attention bindings cannot survive their target item
