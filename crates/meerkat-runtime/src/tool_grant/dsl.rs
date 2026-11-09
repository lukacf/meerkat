// ToolGrantLifecycleMachine production body is catalog-owned. Keep store,
// clock and service mechanics outside this file; the grant lifecycle
// semantics live in the catalog DSL.
use meerkat_machine_schema::catalog::dsl::OptionValueExt;

meerkat_machine_schema::tool_grant_lifecycle_catalog_machine_dsl!(
    "meerkat-runtime",
    "tool_grant::dsl"
);
