use proc_macro2::TokenStream;
use quote::{format_ident, quote};

use crate::ast::{EnumDef, MachineDef, TypeDef};
use crate::gen_state::{gen_type, redacted_debug_value};

/// Whether [`redacted_debug_value`] reads the field (presence or length), so
/// the match arm must bind it.
fn redacted_value_reads_field(ty: &TypeDef) -> bool {
    matches!(
        ty,
        TypeDef::Option(_) | TypeDef::Seq(_) | TypeDef::Set(_) | TypeDef::Map(_, _)
    )
}

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
                quote! {
                    #[allow(clippy::too_many_arguments)]
                    #vname { #(#fields),* }
                }
            }
        })
        .collect();
    let variant_accessors: Vec<_> = enum_def
        .variants
        .iter()
        .map(|v| {
            let vname = &v.name;
            if v.fields.is_empty() {
                quote! { Self::#vname => #variant_name::#vname }
            } else {
                quote! { Self::#vname { .. } => #variant_name::#vname }
            }
        })
        .collect();

    let has_redacted = enum_def
        .variants
        .iter()
        .any(|v| v.fields.iter().any(|f| f.redacted));
    let (derive, debug_impl) = if has_redacted {
        let arms = enum_def.variants.iter().map(|v| {
            let vname = &v.name;
            let vlabel = vname.to_string();
            if v.fields.is_empty() {
                quote! { Self::#vname => f.write_str(#vlabel) }
            } else {
                let bindings = v.fields.iter().map(|field| {
                    let fname = &field.name;
                    if field.redacted && !redacted_value_reads_field(&field.ty) {
                        quote! { #fname: _ }
                    } else {
                        quote! { #fname }
                    }
                });
                let debug_fields = v.fields.iter().map(|field| {
                    let fname = &field.name;
                    let label = fname.to_string();
                    let value = if field.redacted {
                        redacted_debug_value(&quote! { #fname }, &field.ty)
                    } else {
                        quote! { #fname }
                    };
                    quote! { .field(#label, #value) }
                });
                quote! {
                    Self::#vname { #(#bindings),* } => f
                        .debug_struct(#vlabel)
                        #(#debug_fields)*
                        .finish()
                }
            }
        });
        (
            quote! { #[derive(Clone, PartialEq, Eq)] },
            quote! {
                impl std::fmt::Debug for #name {
                    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                        match self {
                            #(#arms),*
                        }
                    }
                }
            },
        )
    } else {
        (quote! { #[derive(Debug, Clone, PartialEq, Eq)] }, quote! {})
    };

    quote! {
        #[allow(clippy::too_many_arguments)]
        #derive
        pub enum #name {
            #(#variants),*
        }

        #debug_impl

        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub enum #variant_name {
            #(#variant_idents),*
        }

        impl #variant_name {
            #[doc(hidden)]
            #[must_use]
            pub const fn as_str(&self) -> &'static str {
                match *self {
                    #(Self::#variant_idents => stringify!(#variant_idents)),*
                }
            }
        }

        impl std::fmt::Display for #variant_name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(self.as_str())
            }
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

            #[doc(hidden)]
            #[must_use]
            pub const fn variant(&self) -> #variant_name {
                match self {
                    #(#variant_accessors),*
                }
            }
        }
    }
}
