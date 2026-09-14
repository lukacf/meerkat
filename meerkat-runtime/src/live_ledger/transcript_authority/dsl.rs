use meerkat_machine_schema::catalog::dsl::OptionValueExt;

meerkat_machine_schema::live_transcript_catalog_machine_dsl!(
    "meerkat-runtime",
    "live_ledger::transcript_authority::dsl"
);
