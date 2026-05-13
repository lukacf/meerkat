// AuthMachine production body is catalog-owned. Keep bridge/runtime mechanics
// outside this file; canonical lifecycle semantics live in the catalog DSL.
use meerkat_core::OAuthProviderIdentity;
use meerkat_machine_schema::catalog::dsl::OptionValueExt;

meerkat_machine_schema::auth_catalog_machine_dsl!("meerkat-runtime", "auth_machine::dsl");
