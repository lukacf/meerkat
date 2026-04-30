use proc_macro2::TokenStream;
use quote::{format_ident, quote};

use crate::ast::{EnumDef, MachineDef};
use crate::gen_state::gen_type;

/// Generate Input, Signal, and Effect enums.
pub fn generate(def: &MachineDef) -> TokenStream {
    let mut output = TokenStream::new();
    output.extend(gen_enum(&def.inputs));
    if !def.signals.variants.is_empty() {
        output.extend(gen_enum(&def.signals));
    }
    if !def.effects.variants.is_empty() {
        output.extend(gen_enum(&def.effects));
    }
    output
}

fn gen_enum(enum_def: &EnumDef) -> TokenStream {
    let name = &enum_def.name;
    let variant_name = format_ident!("{name}Variant");
    let variant_idents: Vec<_> = enum_def
        .variants
        .iter()
        .map(|variant| &variant.name)
        .collect();
    let variants: Vec<_> = enum_def
        .variants
        .iter()
        .map(|v| {
            let vname = &v.name;
            if v.fields.is_empty() {
                quote! { #vname }
            } else {
                let fields: Vec<_> = v
                    .fields
                    .iter()
                    .map(|f| {
                        let fname = &f.name;
                        let fty = gen_type(&f.ty);
                        quote! { #fname: #fty }
                    })
                    .collect();
                quote! { #vname { #(#fields),* } }
            }
        })
        .collect();

    quote! {
        #[derive(Debug, Clone, PartialEq, Eq)]
        pub enum #name {
            #(#variants),*
        }

        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub enum #variant_name {
            #(#variant_idents),*
        }

        impl #name {
            #[doc(hidden)]
            pub const VARIANT_MANIFEST: &'static [#variant_name] = &[
                #(#variant_name::#variant_idents),*
            ];

            #[doc(hidden)]
            #[must_use]
            pub fn variant_manifest() -> &'static [#variant_name] {
                Self::VARIANT_MANIFEST
            }
        }
    }
}
