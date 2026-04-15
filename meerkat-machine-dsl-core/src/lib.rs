mod ast;
mod gen_dispatch;
mod gen_enums;
mod gen_phase;
mod gen_schema;
mod gen_state;
mod parse;
#[cfg(test)]
mod test_machines;
mod validate;

use proc_macro2::TokenStream;
use syn::Error;

/// Expand a `machine! { ... }` invocation into generated Rust code.
///
/// Returns the combined token stream of all generated artifacts, or
/// a compile error with span information pointing at the offending token.
pub fn expand_machine(input: TokenStream) -> Result<TokenStream, Error> {
    let def = parse::parse_machine(input)?;
    validate::validate(&def)?;

    let mut output = TokenStream::new();
    output.extend(gen_state::generate(&def));
    output.extend(gen_phase::generate(&def));
    output.extend(gen_enums::generate(&def));
    output.extend(gen_dispatch::generate(&def));
    output.extend(gen_schema::generate(&def));

    Ok(output)
}
