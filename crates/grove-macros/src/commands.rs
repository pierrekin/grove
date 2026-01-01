//! Implementation of the `#[grove::commands]` attribute macro.
//!
//! This macro is placed on an impl block and:
//! - Finds methods marked with `#[grove(command)]`
//! - Generates a command enum from those methods
//! - Generates the execute() dispatch method
//! - Generates sender methods on the handle

use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::{parse2, Error, FnArg, Ident, ImplItem, ImplItemFn, ItemImpl, Pat, Result, Type};

/// A parsed command method with its information.
struct CommandMethod {
    /// The method name (e.g., `send_message`)
    method_name: Ident,

    /// The PascalCase variant name (e.g., `SendMessage`)
    variant_name: Ident,

    /// The parameter name and type, if any (e.g., `msg: Message`)
    param: Option<(Ident, Type)>,
}

/// Main entry point for the commands attribute macro.
pub fn expand(_attr: TokenStream, item: TokenStream) -> Result<TokenStream> {
    // Parse the impl block
    let impl_block: ItemImpl = parse2(item)?;

    // Get the struct name from the impl
    let struct_name = match &*impl_block.self_ty {
        Type::Path(type_path) => type_path
            .path
            .get_ident()
            .cloned()
            .ok_or_else(|| Error::new_spanned(&impl_block.self_ty, "expected a simple type name"))?,
        _ => {
            return Err(Error::new_spanned(
                &impl_block.self_ty,
                "expected a simple type name",
            ))
        }
    };

    // Derive names
    let command_enum_name = format_ident!("{}Command", struct_name);
    let handle_name = format_ident!("{}Handle", struct_name);

    // Find all methods marked with #[grove(command)]
    let mut command_methods: Vec<CommandMethod> = Vec::new();
    let mut clean_items: Vec<ImplItem> = Vec::new();

    for item in impl_block.items.iter() {
        if let ImplItem::Fn(method) = item {
            if let Some(cmd_method) = parse_command_method(method)? {
                command_methods.push(cmd_method);
                // Keep the method but strip the #[grove(command)] attribute
                let clean_method = strip_grove_attrs(method.clone());
                clean_items.push(ImplItem::Fn(clean_method));
            } else {
                clean_items.push(item.clone());
            }
        } else {
            clean_items.push(item.clone());
        }
    }

    if command_methods.is_empty() {
        return Err(Error::new_spanned(
            &impl_block,
            "no methods marked with #[grove(command)] found",
        ));
    }

    // Generate the command enum
    let enum_variants: Vec<TokenStream> = command_methods
        .iter()
        .map(|cmd| {
            let variant = &cmd.variant_name;
            if let Some((_, ty)) = &cmd.param {
                quote! { #variant(#ty), }
            } else {
                quote! { #variant, }
            }
        })
        .collect();

    let command_enum = quote! {
        /// Auto-generated command enum for the service.
        enum #command_enum_name {
            #(#enum_variants)*
        }
    };

    // Generate the execute impl
    let match_arms: Vec<TokenStream> = command_methods
        .iter()
        .map(|cmd| {
            let variant = &cmd.variant_name;
            let method = &cmd.method_name;
            if cmd.param.is_some() {
                quote! { #command_enum_name::#variant(v) => state.#method(v), }
            } else {
                quote! { #command_enum_name::#variant => state.#method(), }
            }
        })
        .collect();

    let execute_impl = quote! {
        impl #command_enum_name {
            /// Executes this command against the service state.
            pub(crate) fn execute(self, state: &mut #struct_name) {
                match self {
                    #(#match_arms)*
                }
            }
        }
    };

    // Generate handle sender methods
    let sender_methods: Vec<TokenStream> = command_methods
        .iter()
        .map(|cmd| {
            let method = &cmd.method_name;
            let variant = &cmd.variant_name;
            if let Some((param_name, param_ty)) = &cmd.param {
                quote! {
                    /// Sends this command to the service (fire-and-forget).
                    pub fn #method(&self, #param_name: #param_ty) {
                        let _ = self.cmd_tx.try_send(#command_enum_name::#variant(#param_name));
                    }
                }
            } else {
                quote! {
                    /// Sends this command to the service (fire-and-forget).
                    pub fn #method(&self) {
                        let _ = self.cmd_tx.try_send(#command_enum_name::#variant);
                    }
                }
            }
        })
        .collect();

    let handle_impl = quote! {
        impl #handle_name {
            #(#sender_methods)*
        }
    };

    // Reconstruct the impl block without #[grove(command)] attributes
    let impl_generics = &impl_block.generics;
    let where_clause = &impl_block.generics.where_clause;

    let clean_impl = quote! {
        impl #impl_generics #struct_name #where_clause {
            #(#clean_items)*
        }
    };

    // Combine everything
    Ok(quote! {
        #command_enum
        #execute_impl
        #clean_impl
        #handle_impl
    })
}

/// Check if a method has #[grove(command)] and extract its info.
fn parse_command_method(method: &ImplItemFn) -> Result<Option<CommandMethod>> {
    let has_command_attr = method.attrs.iter().any(|attr| {
        if !attr.path().is_ident("grove") {
            return false;
        }
        // Check if it contains "command"
        let mut is_command = false;
        let _ = attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("command") {
                is_command = true;
            }
            Ok(())
        });
        is_command
    });

    if !has_command_attr {
        return Ok(None);
    }

    let method_name = method.sig.ident.clone();

    // Convert snake_case method name to PascalCase variant name
    let variant_name = format_ident!("{}", to_pascal_case(&method_name.to_string()));

    // Extract the parameter (skip &mut self, take the next one if any)
    let param = extract_param(&method.sig.inputs)?;

    Ok(Some(CommandMethod {
        method_name,
        variant_name,
        param,
    }))
}

/// Extract the first non-self parameter from a method signature.
fn extract_param(
    inputs: &syn::punctuated::Punctuated<FnArg, syn::token::Comma>,
) -> Result<Option<(Ident, Type)>> {
    for input in inputs.iter() {
        if let FnArg::Typed(pat_type) = input {
            // Get the parameter name
            let param_name = match &*pat_type.pat {
                Pat::Ident(pat_ident) => pat_ident.ident.clone(),
                _ => continue,
            };
            let param_type = (*pat_type.ty).clone();
            return Ok(Some((param_name, param_type)));
        }
    }
    Ok(None)
}

/// Strip #[grove(...)] attributes from a method.
fn strip_grove_attrs(mut method: ImplItemFn) -> ImplItemFn {
    method.attrs.retain(|attr| !attr.path().is_ident("grove"));
    method
}

/// Convert snake_case to PascalCase.
fn to_pascal_case(s: &str) -> String {
    s.split('_')
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().chain(chars).collect(),
                None => String::new(),
            }
        })
        .collect()
}
