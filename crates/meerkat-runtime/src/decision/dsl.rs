// DurableDecisionMachine production body is catalog-owned. Keep the owner
// service, the store and the execution permit outside this file; canonical
// decision lifecycle semantics live in the catalog DSL.
use meerkat_machine_schema::catalog::dsl::OptionValueExt;

meerkat_machine_schema::durable_decision_catalog_machine_dsl!("meerkat-runtime", "decision::dsl");
