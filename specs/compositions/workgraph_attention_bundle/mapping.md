# workgraph_attention_bundle Mapping Note

<!-- GENERATED_COVERAGE_START -->
## Generated Coverage
This section is generated from the Rust composition catalog. Do not edit it by hand.

### Composition
- `workgraph_attention_bundle`

### Code Anchors
- `workgraph_attention_service_close`: `meerkat-workgraph/src/service.rs` — WorkGraph service close path realizes the canonical WorkGraph Closed to WorkAttention Stop route with an atomic item-and-attention CAS update
- `workgraph_attention_bundle_schema`: `meerkat-machine-schema/src/catalog/compositions.rs` — formal WorkGraph item closure to WorkAttention stop composition

### Scenarios
- `close-stops-attention` — terminal WorkGraph item closure routes to WorkAttention Stop so live goal attention bindings cannot survive their target item

### Routes
- `work_item_close_stops_attention`
  - anchors: `workgraph_attention_service_close`
  - scenarios: `close-stops-attention`

### Scheduler Rules
- `(none)`

### Invariants
- `closed_work_item_routes_to_attention_stop`
  - anchors: `workgraph_attention_service_close`
  - scenarios: `close-stops-attention`
- `attention_stop_originates_from_work_item_close`
  - anchors: `workgraph_attention_service_close`
  - scenarios: `close-stops-attention`


<!-- GENERATED_COVERAGE_END -->
