//! Implementation of the `#[grove::service]` attribute macro.
//!
//! This attribute macro:
//! - Optionally injects an emitter field for services that emit events
//! - Generates the handle struct
//! - Generates getters and subscription methods
//! - Generates emit_<event>() methods

use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::{
    parse2, Data, DeriveInput, Error, Field, Fields, FieldsNamed, Ident, Result, Type,
};

/// Main entry point for the service attribute macro.
pub fn expand(_attr: TokenStream, item: TokenStream) -> Result<TokenStream> {
    let mut input: DeriveInput = parse2(item)?;

    // Parse struct-level attributes for emits = [...]
    let emitted_events = parse_emits(&input)?;

    // Get mutable access to fields
    let fields = match &mut input.data {
        Data::Struct(data) => match &mut data.fields {
            Fields::Named(fields) => fields,
            _ => {
                return Err(Error::new_spanned(
                    &input,
                    "grove::service can only be applied to structs with named fields",
                ))
            }
        },
        _ => {
            return Err(Error::new_spanned(
                &input,
                "grove::service can only be applied to structs",
            ))
        }
    };

    let struct_name = &input.ident;
    let handle_name = format_ident!("{}Handle", struct_name);
    let command_type = format_ident!("{}Command", struct_name);

    // Parse field attributes for getters BEFORE injecting emitter
    let getters: Vec<GetterField> = fields
        .named
        .iter()
        .filter_map(|f| GetterField::from_field(f).transpose())
        .collect::<Result<_>>()?;

    // Collect all user-declared fields BEFORE injecting emitter
    let user_fields: Vec<_> = fields
        .named
        .iter()
        .map(|f| (f.ident.clone().unwrap(), f.ty.clone()))
        .collect();

    // Inject emitter field if this service emits events (AFTER collecting user fields)
    let has_emitter = !emitted_events.is_empty();
    if has_emitter {
        inject_emitter_field(fields);
    }

    // Generate code
    let handle_struct = generate_handle_struct(struct_name, &handle_name, &command_type);
    let getter_impls = generate_getters(&handle_name, &getters);
    let subscription_methods = generate_subscription_methods(&handle_name, &emitted_events);
    let emit_methods = generate_emit_methods(struct_name, &emitted_events);
    let constructor = generate_constructor(struct_name, &user_fields, has_emitter);

    // Remove grove attributes from struct (they've been processed)
    input.attrs.retain(|attr| !attr.path().is_ident("grove"));

    // Remove grove attributes from fields
    if let Data::Struct(data) = &mut input.data {
        if let Fields::Named(fields) = &mut data.fields {
            for field in &mut fields.named {
                field.attrs.retain(|attr| !attr.path().is_ident("grove"));
            }
        }
    }

    Ok(quote! {
        #input
        #handle_struct
        #getter_impls
        #subscription_methods
        #emit_methods
        #constructor
    })
}

/// Parse #[grove(emits = [Event1, Event2])] from struct attributes.
fn parse_emits(input: &DeriveInput) -> Result<Vec<Ident>> {
    let mut events = Vec::new();

    for attr in &input.attrs {
        if !attr.path().is_ident("grove") {
            continue;
        }

        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("emits") {
                meta.input.parse::<syn::Token![=]>()?;

                // Parse [Event1, Event2, ...]
                let content;
                syn::bracketed!(content in meta.input);

                while !content.is_empty() {
                    let event: Ident = content.parse()?;
                    events.push(event);

                    if content.is_empty() {
                        break;
                    }
                    content.parse::<syn::Token![,]>()?;
                }
                Ok(())
            } else {
                Ok(())
            }
        })?;
    }

    Ok(events)
}

/// Inject a hidden emitter field into the struct with a default value.
fn inject_emitter_field(fields: &mut FieldsNamed) {
    // We can't give struct fields default values directly in Rust.
    // Instead, we'll implement Default for the struct or use a different approach.
    // For now, we document that users should use ..Default::default() or a builder.
    let emitter_field: syn::Field = syn::parse_quote! {
        #[doc(hidden)]
        pub __grove_emitter: grove::event::Emitter
    };
    fields.named.push(emitter_field);
}

// ============================================================================
// Field Parsing
// ============================================================================

struct GetterField {
    name: Ident,
    ty: Type,
}

impl GetterField {
    fn from_field(field: &Field) -> Result<Option<Self>> {
        let name = field
            .ident
            .clone()
            .expect("we already verified these are named fields");

        // Skip the injected emitter field
        if name == "__grove_emitter" {
            return Ok(None);
        }

        for attr in &field.attrs {
            if !attr.path().is_ident("grove") {
                continue;
            }

            let mut is_getter = false;

            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("get") {
                    is_getter = true;
                }
                Ok(())
            })?;

            if is_getter {
                return Ok(Some(Self {
                    name,
                    ty: field.ty.clone(),
                }));
            }
        }

        Ok(None)
    }
}

// ============================================================================
// Code Generation
// ============================================================================

fn generate_handle_struct(
    struct_name: &Ident,
    handle_name: &Ident,
    command_type: &Ident,
) -> TokenStream {
    quote! {
        /// Handle for interacting with the service.
        #[derive(Clone)]
        pub struct #handle_name {
            state: grove::runtime::Arc<grove::runtime::RwLock<#struct_name>>,
            cmd_tx: grove::runtime::mpsc::Sender<#command_type>,
        }
    }
}

fn generate_getters(handle_name: &Ident, getters: &[GetterField]) -> TokenStream {
    let getter_methods: Vec<TokenStream> = getters
        .iter()
        .map(|getter| {
            let field_name = &getter.name;
            let field_type = &getter.ty;

            quote! {
                pub fn #field_name(&self) -> #field_type {
                    self.state.read().unwrap().#field_name.clone()
                }
            }
        })
        .collect();

    if getter_methods.is_empty() {
        return quote! {};
    }

    quote! {
        impl #handle_name {
            #(#getter_methods)*
        }
    }
}

fn generate_subscription_methods(handle_name: &Ident, emitted_events: &[Ident]) -> TokenStream {
    if emitted_events.is_empty() {
        return quote! {};
    }

    let methods: Vec<TokenStream> = emitted_events
        .iter()
        .map(|event| {
            let method_name = format_ident!("on_{}", to_snake_case(&event.to_string()));

            quote! {
                /// Subscribe to this event type.
                pub fn #method_name(&self) -> grove::runtime::broadcast::Receiver<#event> {
                    self.state.read().unwrap().__grove_emitter.subscribe::<#event>()
                }
            }
        })
        .collect();

    quote! {
        impl #handle_name {
            #(#methods)*
        }
    }
}

/// Generate the `new()` constructor that initializes all fields.
fn generate_constructor(
    struct_name: &Ident,
    user_fields: &[(Ident, Type)],
    has_emitter: bool,
) -> TokenStream {
    let params: Vec<TokenStream> = user_fields
        .iter()
        .map(|(name, ty)| quote! { #name: #ty })
        .collect();

    let field_inits: Vec<TokenStream> = user_fields
        .iter()
        .map(|(name, _)| quote! { #name })
        .collect();

    let emitter_init = if has_emitter {
        quote! { __grove_emitter: grove::event::Emitter::new(), }
    } else {
        quote! {}
    };

    quote! {
        impl #struct_name {
            /// Creates a new instance of this service.
            pub fn new(#(#params),*) -> Self {
                Self {
                    #(#field_inits,)*
                    #emitter_init
                }
            }
        }
    }
}

/// Generate emit_<event>() methods and __wire_emitter helper.
fn generate_emit_methods(struct_name: &Ident, emitted_events: &[Ident]) -> TokenStream {
    if emitted_events.is_empty() {
        // No events - generate a no-op wire_emitter so handlers can always call it
        return quote! {
            impl #struct_name {
                #[doc(hidden)]
                pub fn __wire_emitter(&mut self) {}
            }
        };
    }

    let emit_methods: Vec<TokenStream> = emitted_events
        .iter()
        .map(|event| {
            let method_name = format_ident!("emit_{}", to_snake_case(&event.to_string()));

            quote! {
                /// Emit this event to all subscribers.
                fn #method_name(&self, event: #event) {
                    self.__grove_emitter.emit(event);
                }
            }
        })
        .collect();

    // Generate the wire_emitter helper that spawn() will call
    let channel_creation: Vec<TokenStream> = emitted_events
        .iter()
        .map(|event| {
            quote! {
                builder.add_event::<#event>(256);
            }
        })
        .collect();

    quote! {
        impl #struct_name {
            #(#emit_methods)*

            /// Wires up the event channels. Called automatically by spawn().
            #[doc(hidden)]
            pub fn __wire_emitter(&mut self) {
                let mut builder = grove::event::EmitterBuilder::new();
                #(#channel_creation)*
                self.__grove_emitter = builder.build();
            }
        }
    }
}

fn to_snake_case(s: &str) -> String {
    let mut result = String::new();
    for (i, c) in s.chars().enumerate() {
        if c.is_uppercase() {
            if i > 0 {
                result.push('_');
            }
            result.push(c.to_lowercase().next().unwrap());
        } else {
            result.push(c);
        }
    }
    result
}
