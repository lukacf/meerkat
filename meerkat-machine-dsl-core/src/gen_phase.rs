use proc_macro2::TokenStream;
use quote::quote;

use crate::ast::MachineDef;
use crate::gen_state::state_struct_name;

/// Generate the phase enum and phase() projection method.
pub fn generate(def: &MachineDef) -> TokenStream {
    let phase_name = &def.phase_enum.name;
    let variants: Vec<_> = def.phase_enum.variants.iter().collect();

    let phase_method = if let Some(phase_field) = def.phase_field_name() {
        quote! {
            pub fn phase(&self) -> #phase_name {
                self.#phase_field
            }
        }
    } else if let Some(proj) = &def.phase_projection {
        let arms: Vec<_> = proj
            .rules
            .iter()
            .map(|rule| {
                let phase = &rule.phase;
                if let Some(cond) = &rule.condition {
                    let cond_tokens = crate::gen_dispatch::gen_expr(
                        cond,
                        crate::gen_dispatch::FieldPrefix::DirectSelf,
                    );
                    quote! {
                        if #cond_tokens { return #phase_name::#phase; }
                    }
                } else {
                    // Fallback — no condition
                    quote! {
                        return #phase_name::#phase;
                    }
                }
            })
            .collect();
        quote! {
            pub fn phase(&self) -> #phase_name {
                #(#arms)*
                unreachable!("phase projection must be exhaustive")
            }
        }
    } else {
        // No projection and not stored — should be caught by validation
        quote! {
            pub fn phase(&self) -> #phase_name {
                unreachable!("no phase projection defined")
            }
        }
    };

    let state_name = state_struct_name(def);

    quote! {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum #phase_name {
            #(#variants),*
        }

        impl #state_name {
            #phase_method
        }
    }
}
