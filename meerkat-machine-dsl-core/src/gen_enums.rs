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
    let kind_name = format_ident!("{}Kind", name);
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

    let kind_variants: Vec<_> = enum_def.variants.iter().map(|v| &v.name).collect();
    let kind_arms: Vec<_> = enum_def
        .variants
        .iter()
        .map(|v| {
            let vname = &v.name;
            let pattern = if v.fields.is_empty() {
                quote! { Self::#vname }
            } else {
                quote! { Self::#vname { .. } }
            };
            quote! { #pattern => #kind_name::#vname }
        })
        .collect();

    quote! {
        #[derive(Debug, Clone, PartialEq, Eq)]
        pub enum #name {
            #(#variants),*
        }

        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum #kind_name {
            #(#kind_variants),*
        }

        impl #name {
            pub fn kind(&self) -> #kind_name {
                match self {
                    #(#kind_arms),*
                }
            }
        }
    }
}
