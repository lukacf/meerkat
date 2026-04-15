use syn::Error;

use crate::ast::MachineDef;

/// Validate the parsed machine definition for semantic correctness.
pub fn validate(def: &MachineDef) -> Result<(), Error> {
    // TODO: implement validation
    // - field references in guards/updates exist in state block
    // - phase references in transitions exist in phase enum
    // - transition names are unique
    // - bindings in triggers match input/signal variant fields
    // - terminal phases are a subset of phase enum variants
    // - stored-phase: phase field type matches phase enum name
    // - derived-phase: phase_projection block is present and covers all phases
    // - invariant expressions reference valid fields/helpers
    // - disposition effects exist in effect enum
    let _ = def;
    Ok(())
}
