use proc_macro2::TokenStream;
use quote::quote;

use crate::ast::{MachineDef, TypeDef};

/// Generate the state struct and Default impl.
pub fn generate(def: &MachineDef) -> TokenStream {
    let state_name = state_struct_name(def);
    let fields: Vec<_> = def
        .state_fields
        .iter()
        .map(|f| {
            let name = &f.name;
            let ty = gen_type(&f.ty);
            quote! { pub #name: #ty }
        })
        .collect();

    let default_fields: Vec<_> = if let Some(phase_field) = def.phase_field_name() {
        let phase_enum_name = &def.phase_enum.name;
        let init_phase = &def.init_phase;

        let mut defaults: Vec<_> = vec![quote! { #phase_field: #phase_enum_name::#init_phase }];
        for init in &def.init_fields {
            let name = &init.name;
            let value = gen_init_value(&init.value);
            defaults.push(quote! { #name: #value });
        }
        defaults
    } else {
        def.init_fields
            .iter()
            .map(|init| {
                let name = &init.name;
                let value = gen_init_value(&init.value);
                quote! { #name: #value }
            })
            .collect()
    };

    let state_label = state_name.to_string();
    let (derive, debug_impl) = if def.state_fields.iter().any(|f| f.redacted) {
        let debug_fields = def.state_fields.iter().map(|f| {
            let name = &f.name;
            let label = name.to_string();
            let value = if f.redacted {
                redacted_debug_value(&quote! { self.#name }, &f.ty)
            } else {
                quote! { &self.#name }
            };
            quote! { .field(#label, #value) }
        });
        (
            quote! { #[derive(Clone, PartialEq, Eq)] },
            quote! {
                impl std::fmt::Debug for #state_name {
                    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                        f.debug_struct(#state_label)
                            #(#debug_fields)*
                            .finish()
                    }
                }
            },
        )
    } else {
        (quote! { #[derive(Debug, Clone, PartialEq, Eq)] }, quote! {})
    };

    quote! {
        #derive
        pub struct #state_name {
            #(#fields),*
        }

        #debug_impl

        impl Default for #state_name {
            fn default() -> Self {
                Self {
                    #(#default_fields),*
                }
            }
        }
    }
}

/// `Debug` value for a `#[redacted]` field reached through `access`: presence
/// for an option, the entry count for a collection, otherwise a marker. The
/// value itself never reaches the formatter.
pub(crate) fn redacted_debug_value(access: &TokenStream, ty: &TypeDef) -> TokenStream {
    match ty {
        TypeDef::Option(_) => quote! { &#access.as_ref().map(|_| "<redacted>") },
        TypeDef::Seq(_) | TypeDef::Set(_) | TypeDef::Map(_, _) => {
            quote! { &format_args!("<redacted; {} entries>", #access.len()) }
        }
        TypeDef::Bool
        | TypeDef::U32
        | TypeDef::U64
        | TypeDef::String
        | TypeDef::Named(_)
        | TypeDef::Enum(_) => quote! { &"<redacted>" },
    }
}

pub(crate) fn state_struct_name(def: &MachineDef) -> syn::Ident {
    syn::Ident::new(&format!("{}State", def.name), def.name.span())
}

pub(crate) fn gen_type(ty: &TypeDef) -> TokenStream {
    match ty {
        TypeDef::Bool => quote! { bool },
        TypeDef::U32 => quote! { u32 },
        TypeDef::U64 => quote! { u64 },
        TypeDef::String => quote! { String },
        TypeDef::Option(inner) => {
            let inner_ty = gen_type(inner);
            quote! { Option<#inner_ty> }
        }
        TypeDef::Seq(inner) => {
            let inner_ty = gen_type(inner);
            quote! { Vec<#inner_ty> }
        }
        TypeDef::Set(inner) => {
            let inner_ty = gen_type(inner);
            quote! { std::collections::BTreeSet<#inner_ty> }
        }
        TypeDef::Map(k, v) => {
            let key_ty = gen_type(k);
            let val_ty = gen_type(v);
            quote! { std::collections::BTreeMap<#key_ty, #val_ty> }
        }
        TypeDef::Named(ident) => quote! { #ident },
        TypeDef::Enum(ident) => quote! { #ident },
    }
}

fn gen_init_value(expr: &crate::ast::ExprDef) -> TokenStream {
    use crate::ast::ExprDef;
    match expr {
        ExprDef::Bool(v) => quote! { #v },
        ExprDef::U64(v) => quote! { #v },
        ExprDef::U64Max => quote! { u64::MAX },
        ExprDef::StringLit(s) => quote! { #s.into() },
        ExprDef::None => quote! { None },
        ExprDef::Some(inner) => {
            let inner_val = gen_init_value(inner);
            quote! { Some(#inner_val) }
        }
        ExprDef::EmptySeq => quote! { Vec::new() },
        ExprDef::EmptySet => quote! { std::collections::BTreeSet::new() },
        ExprDef::EmptyMap => quote! { std::collections::BTreeMap::new() },
        ExprDef::NamedVariant { enum_name, variant } => quote! { #enum_name::#variant },
        ExprDef::Phase(variant) => {
            // Phase variant in init — for stored-phase machines
            quote! { Phase::#variant }
        }
        _ => quote! { Default::default() },
    }
}
