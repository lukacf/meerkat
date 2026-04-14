use proc_macro::TokenStream;
use quote::quote;
use syn::{Data, DeriveInput, Fields, parse_macro_input};

#[proc_macro_derive(CommandManifest)]
pub fn derive_command_manifest(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    let enum_ident = input.ident;

    let Data::Enum(data_enum) = input.data else {
        return syn::Error::new_spanned(
            enum_ident,
            "CommandManifest can only be derived for enums",
        )
        .to_compile_error()
        .into();
    };

    let mut variant_names = Vec::with_capacity(data_enum.variants.len());
    for variant in data_enum.variants {
        match variant.fields {
            Fields::Named(_) | Fields::Unnamed(_) | Fields::Unit => {
                variant_names.push(variant.ident.to_string());
            }
        }
    }

    let expanded = quote! {
        impl #enum_ident {
            #[doc(hidden)]
            pub const COMMAND_MANIFEST: &'static [&'static str] = &[
                #( #variant_names ),*
            ];

            #[doc(hidden)]
            #[must_use]
            pub fn command_manifest() -> &'static [&'static str] {
                Self::COMMAND_MANIFEST
            }
        }
    };

    expanded.into()
}
