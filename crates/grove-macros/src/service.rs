//! Implementation of the `#[derive(Service)]` macro.
//!
//! This module contains all the logic for parsing the input struct and
//! generating the actor infrastructure.

use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::{Data, DeriveInput, Error, Field, Fields, Ident, Result, Type};

/// Main entry point for the Service derive macro.
///
/// This function orchestrates the code generation:
/// 1. Validate we're deriving on a struct (not enum/union)
/// 2. Parse field attributes for getters
/// 3. Generate the handle struct, spawn method, and getters
pub fn expand(input: DeriveInput) -> Result<TokenStream> {
    // Step 1: Extract the struct fields.
    // We only support structs with named fields (not tuple structs or unit structs).
    let fields = match &input.data {
        Data::Struct(data) => match &data.fields {
            Fields::Named(fields) => &fields.named,
            _ => {
                return Err(Error::new_spanned(
                    &input,
                    "Service can only be derived for structs with named fields",
                ))
            }
        },
        _ => {
            return Err(Error::new_spanned(
                &input,
                "Service can only be derived for structs",
            ))
        }
    };

    // Step 2: Derive names from the struct name
    let struct_name = &input.ident;
    let handle_name = format_ident!("{}Handle", struct_name);
    let command_type = format_ident!("{}Command", struct_name);

    // Step 3: Parse field attributes - find getters
    let getters: Vec<GetterField> = fields
        .iter()
        .filter_map(|f| GetterField::from_field(f).transpose())
        .collect::<Result<_>>()?;

    // Step 4: Generate all the code
    let handle_struct = generate_handle_struct(struct_name, &handle_name, &command_type);
    let spawn_impl = generate_spawn_impl(struct_name, &handle_name, &command_type);
    let getter_impls = generate_getters(&handle_name, &getters);

    // Combine all generated code into a single token stream
    Ok(quote! {
        #handle_struct
        #spawn_impl
        #getter_impls
    })
}

// ============================================================================
// Field Parsing
// ============================================================================

/// A field marked with `#[grove(get)]` that needs a getter generated.
struct GetterField {
    /// The field name (e.g., `messages`)
    name: Ident,

    /// The field type (e.g., `Vec<Message>`)
    ty: Type,
}

impl GetterField {
    /// Check if a field has `#[grove(get)]` and extract its info.
    ///
    /// Fields can have:
    /// - `#[grove(get)]` - generate a getter
    /// - `#[grove(skip)]` - no getter (for injected handles)
    /// - No attribute - no getter
    ///
    /// Returns:
    /// - `Ok(Some(GetterField))` if the field has `#[grove(get)]`
    /// - `Ok(None)` if the field has no grove attributes or has `skip`
    /// - `Err(...)` if the attribute is malformed
    fn from_field(field: &Field) -> Result<Option<Self>> {
        let name = field
            .ident
            .clone()
            .expect("we already verified these are named fields");

        for attr in &field.attrs {
            if !attr.path().is_ident("grove") {
                continue;
            }

            let mut is_getter = false;
            let mut is_skip = false;

            // Handle #[grove(get)] or #[grove(skip)] syntax
            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("get") {
                    is_getter = true;
                    Ok(())
                } else if meta.path.is_ident("skip") {
                    is_skip = true;
                    Ok(())
                } else {
                    Err(meta.error("unknown field attribute, expected `get` or `skip`"))
                }
            })?;

            if is_getter && is_skip {
                return Err(Error::new_spanned(
                    attr,
                    "field cannot have both `get` and `skip`",
                ));
            }

            if is_getter {
                return Ok(Some(Self {
                    name,
                    ty: field.ty.clone(),
                }));
            }

            // If skip, return None (no getter)
            if is_skip {
                return Ok(None);
            }
        }

        Ok(None)
    }
}

// ============================================================================
// Code Generation
// ============================================================================

/// Generate the handle struct.
fn generate_handle_struct(
    struct_name: &Ident,
    handle_name: &Ident,
    command_type: &Ident,
) -> TokenStream {
    quote! {
        /// Handle for interacting with the service.
        ///
        /// This struct is cheaply cloneable and can be passed to multiple tasks.
        /// - Read operations go directly through the `RwLock` (fast, synchronous)
        /// - Write operations are sent through the command channel (async, serialized)
        #[derive(Clone)]
        pub struct #handle_name {
            state: grove::runtime::Arc<grove::runtime::RwLock<#struct_name>>,
            cmd_tx: grove::runtime::mpsc::Sender<#command_type>,
        }
    }
}

/// Generate the spawn method as an associated function on the struct.
fn generate_spawn_impl(
    struct_name: &Ident,
    handle_name: &Ident,
    command_type: &Ident,
) -> TokenStream {
    quote! {
        impl #struct_name {
            /// Spawns the service and returns a handle for interacting with it.
            ///
            /// This method:
            /// 1. Wraps the initial state in `Arc<RwLock<_>>`
            /// 2. Sets up the command channel
            /// 3. Spawns the async service loop
            /// 4. Returns a handle for sending commands and reading state
            pub fn spawn(self) -> #handle_name {
                let state = grove::runtime::Arc::new(
                    grove::runtime::RwLock::new(self)
                );
                let (cmd_tx, mut cmd_rx) = grove::runtime::mpsc::channel::<#command_type>(32);

                let handle = #handle_name {
                    state: state.clone(),
                    cmd_tx,
                };

                // Spawn the service loop.
                // Command dispatch is generated by #[grove::commands].
                tokio::spawn(async move {
                    while let Some(cmd) = cmd_rx.recv().await {
                        let mut state = state.write().unwrap();
                        cmd.execute(&mut state);
                    }
                });

                handle
            }
        }
    }
}

/// Generate getter methods for fields marked with `#[grove(get)]`.
fn generate_getters(handle_name: &Ident, getters: &[GetterField]) -> TokenStream {
    let getter_methods: Vec<TokenStream> = getters
        .iter()
        .map(|getter| {
            let field_name = &getter.name;
            let field_type = &getter.ty;

            quote! {
                /// Returns a clone of the current value.
                ///
                /// This reads directly from the shared state (no channel overhead).
                /// Note: The value may not reflect pending commands that haven't been processed yet.
                pub fn #field_name(&self) -> #field_type {
                    self.state.read().unwrap().#field_name.clone()
                }
            }
        })
        .collect();

    quote! {
        impl #handle_name {
            #(#getter_methods)*
        }
    }
}
