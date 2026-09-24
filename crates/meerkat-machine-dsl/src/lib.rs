#![allow(clippy::redundant_feature_names)]

use proc_macro::TokenStream;

/// Define a Meerkat state machine from a single DSL definition.
///
/// Generates: state struct, phase enum, input/signal/effect enums,
/// sealed dispatch function, schema artifact, and helper/invariant code.
///
/// See `meerkat-machine-dsl-core` for the full DSL syntax reference.
#[proc_macro]
pub fn machine(input: TokenStream) -> TokenStream {
    meerkat_machine_dsl_core::expand_machine(input.into())
        .unwrap_or_else(|err| err.to_compile_error())
        .into()
}
