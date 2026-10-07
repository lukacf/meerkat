// The canonical declaration is shared with schema/model generation.
// The current generic expression renderer clones owned literals and projections;
// this generated-only allowance does not cover the handwritten host.
#![allow(clippy::cmp_owned)]
meerkat_machine_schema::grant_authority_catalog_machine_dsl!(
    "meerkat-authorization",
    "grants::dsl"
);
