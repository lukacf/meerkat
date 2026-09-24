use proc_macro::TokenStream;
use quote::{format_ident, quote};
use syn::{Data, DeriveInput, parse_macro_input};

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
    let mut variant_idents = Vec::with_capacity(data_enum.variants.len());
    for variant in data_enum.variants {
        variant_names.push(variant.ident.to_string());
        variant_idents.push(variant.ident);
    }
    let variant_enum_ident = format_ident!("{enum_ident}Variant");

    let expanded = quote! {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub enum #variant_enum_ident {
            #( #variant_idents ),*
        }

        impl #variant_enum_ident {
            #[doc(hidden)]
            #[must_use]
            pub const fn as_str(self) -> &'static str {
                match self {
                    #( Self::#variant_idents => stringify!(#variant_idents), )*
                }
            }
        }

        impl #enum_ident {
            #[doc(hidden)]
            pub const COMMAND_MANIFEST: &'static [&'static str] = &[
                #( #variant_names ),*
            ];

            #[doc(hidden)]
            pub const COMMAND_VARIANT_MANIFEST: &'static [#variant_enum_ident] = &[
                #( #variant_enum_ident::#variant_idents ),*
            ];

            #[doc(hidden)]
            #[must_use]
            pub fn command_manifest() -> &'static [&'static str] {
                Self::COMMAND_MANIFEST
            }

            #[doc(hidden)]
            #[must_use]
            pub fn command_variant_manifest() -> &'static [#variant_enum_ident] {
                Self::COMMAND_VARIANT_MANIFEST
            }
        }
    };

    expanded.into()
}
