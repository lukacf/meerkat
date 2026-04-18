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

    quote! {
        #[derive(Debug, Clone, PartialEq, Eq)]
        pub struct #state_name {
            #(#fields),*
        }

        impl Default for #state_name {
            fn default() -> Self {
                Self {
                    #(#default_fields),*
                }
            }
        }
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
        ExprDef::StringLit(s) => quote! { #s.into() },
        ExprDef::None => quote! { None },
        ExprDef::Some(inner) => {
            let inner_val = gen_init_value(inner);
            quote! { Some(#inner_val) }
        }
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
