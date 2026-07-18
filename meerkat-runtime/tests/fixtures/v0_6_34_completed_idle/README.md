# v0.6.34 completed-idle persistence fixture

These frozen schema and row fixtures pin the persistence shapes emitted by Meerkat tag
`v0.6.34` at commit `d55483ec8e1f18a9a8377f6e1e2e501664204ada`:

- `meerkat-runtime/src/store/sqlite.rs` table layout and row encoding
- `meerkat-runtime/src/input_state.rs` versionless `InputStateSerde`
- `meerkat-runtime/src/ops_lifecycle.rs` pre-generated-feed ops snapshot
- `meerkat-core/src/session.rs` session envelope v2
- `meerkat-core/src/lifecycle/run_receipt.rs` boundary receipt

`schema.sql`, `realm_manifest.json`, and `session_row.json` make this a complete
realm fixture rather than an isolated runtime-state sample. The input
intentionally retains the retired `PromptInput { text, blocks }`
carrier. Migration must preserve these original bytes in immutable audit data,
but must not promote that retired payload into current active-work authority.
