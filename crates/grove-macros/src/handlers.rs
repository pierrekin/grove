//! Implementation of the `#[grove::handlers]` attribute macro.
//!
//! This macro is placed on an impl block and:
//! - Finds methods marked with `#[grove(command)]` - generates command handling
//! - Finds methods marked with `#[grove(from = field)]` - generates event subscriptions
//! - Generates spawn() method with select! for commands and events

use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::{parse2, Error, FnArg, Ident, ImplItem, ImplItemFn, ItemImpl, Pat, Result, Type};

/// A parsed command method.
struct CommandMethod {
    method_name: Ident,
    variant_name: Ident,
    param: Option<(Ident, Type)>,
}

/// A parsed event handler method.
struct EventHandler {
    /// The method name (e.g., `on_message_sent`)
    method_name: Ident,
    /// The field that provides the subscription (e.g., `chat`)
    source_field: Ident,
    /// The event type (extracted from method parameter)
    event_type: Type,
}

/// Main entry point for the handlers attribute macro.
pub fn expand(_attr: TokenStream, item: TokenStream) -> Result<TokenStream> {
    let impl_block: ItemImpl = parse2(item)?;

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

    let command_enum_name = format_ident!("{}Command", struct_name);
    let handle_name = format_ident!("{}Handle", struct_name);

    // Parse all methods
    let mut command_methods: Vec<CommandMethod> = Vec::new();
    let mut event_handlers: Vec<EventHandler> = Vec::new();
    let mut clean_items: Vec<ImplItem> = Vec::new();

    for item in impl_block.items.iter() {
        if let ImplItem::Fn(method) = item {
            if let Some(cmd) = parse_command_method(method)? {
                command_methods.push(cmd);
                clean_items.push(ImplItem::Fn(strip_grove_attrs(method.clone())));
            } else if let Some(handler) = parse_event_handler(method)? {
                event_handlers.push(handler);
                clean_items.push(ImplItem::Fn(strip_grove_attrs(method.clone())));
            } else {
                clean_items.push(item.clone());
            }
        } else {
            clean_items.push(item.clone());
        }
    }

    // Generate command infrastructure (always generate enum, even if empty)
    let (command_enum, execute_impl, handle_sender_impl) =
        generate_command_infra(&struct_name, &command_enum_name, &handle_name, &command_methods);

    // Generate spawn implementation
    let spawn_impl = generate_spawn_impl(
        &struct_name,
        &handle_name,
        &command_enum_name,
        &command_methods,
        &event_handlers,
    );

    // Reconstruct the impl block
    let impl_generics = &impl_block.generics;
    let where_clause = &impl_block.generics.where_clause;

    let clean_impl = quote! {
        impl #impl_generics #struct_name #where_clause {
            #(#clean_items)*
        }
    };

    Ok(quote! {
        #command_enum
        #execute_impl
        #clean_impl
        #handle_sender_impl
        #spawn_impl
    })
}

// ============================================================================
// Parsing
// ============================================================================

fn parse_command_method(method: &ImplItemFn) -> Result<Option<CommandMethod>> {
    let has_command_attr = method.attrs.iter().any(|attr| {
        if !attr.path().is_ident("grove") {
            return false;
        }
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
    let variant_name = format_ident!("{}", to_pascal_case(&method_name.to_string()));
    let param = extract_param(&method.sig.inputs)?;

    Ok(Some(CommandMethod {
        method_name,
        variant_name,
        param,
    }))
}

fn parse_event_handler(method: &ImplItemFn) -> Result<Option<EventHandler>> {
    let mut source_field: Option<Ident> = None;

    for attr in &method.attrs {
        if !attr.path().is_ident("grove") {
            continue;
        }

        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("from") {
                meta.input.parse::<syn::Token![=]>()?;
                let field: Ident = meta.input.parse()?;
                source_field = Some(field);
            }
            Ok(())
        })?;
    }

    let source_field = match source_field {
        Some(f) => f,
        None => return Ok(None),
    };

    // Extract event type from method parameter
    let (_, event_type) = extract_param(&method.sig.inputs)?
        .ok_or_else(|| Error::new_spanned(method, "event handler must have an event parameter"))?;

    Ok(Some(EventHandler {
        method_name: method.sig.ident.clone(),
        source_field,
        event_type,
    }))
}

fn extract_param(
    inputs: &syn::punctuated::Punctuated<FnArg, syn::token::Comma>,
) -> Result<Option<(Ident, Type)>> {
    for input in inputs.iter() {
        if let FnArg::Typed(pat_type) = input {
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

fn strip_grove_attrs(mut method: ImplItemFn) -> ImplItemFn {
    method.attrs.retain(|attr| !attr.path().is_ident("grove"));
    method
}

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

/// Derive subscription method name from event type (e.g., MessageSent -> on_message_sent)
fn derive_subscription_method(event_type: &Type) -> Ident {
    // Extract the type name from the Type
    let type_name = match event_type {
        Type::Path(type_path) => type_path
            .path
            .segments
            .last()
            .map(|seg| seg.ident.to_string())
            .unwrap_or_default(),
        _ => String::new(),
    };

    let snake_name = to_snake_case(&type_name);
    format_ident!("on_{}", snake_name)
}

// ============================================================================
// Code Generation
// ============================================================================

fn generate_command_infra(
    struct_name: &Ident,
    command_enum_name: &Ident,
    handle_name: &Ident,
    commands: &[CommandMethod],
) -> (TokenStream, TokenStream, TokenStream) {
    // Generate enum variants
    let enum_variants: Vec<TokenStream> = commands
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
        enum #command_enum_name {
            #(#enum_variants)*
        }
    };

    // Generate execute impl
    let match_arms: Vec<TokenStream> = commands
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
            fn execute(self, state: &mut #struct_name) {
                match self {
                    #(#match_arms)*
                }
            }
        }
    };

    // Generate handle sender methods
    let sender_methods: Vec<TokenStream> = commands
        .iter()
        .map(|cmd| {
            let method = &cmd.method_name;
            let variant = &cmd.variant_name;
            if let Some((param_name, param_ty)) = &cmd.param {
                quote! {
                    pub fn #method(&self, #param_name: #param_ty) {
                        let _ = self.cmd_tx.try_send(#command_enum_name::#variant(#param_name));
                    }
                }
            } else {
                quote! {
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

    (command_enum, execute_impl, handle_impl)
}

fn generate_spawn_impl(
    struct_name: &Ident,
    handle_name: &Ident,
    command_enum_name: &Ident,
    commands: &[CommandMethod],
    event_handlers: &[EventHandler],
) -> TokenStream {
    // Generate event receiver setup
    // The subscription method name is derived from the event type (e.g., MessageSent -> on_message_sent)
    let event_receiver_setup: Vec<TokenStream> = event_handlers
        .iter()
        .map(|handler| {
            let field = &handler.source_field;
            let handler_method = &handler.method_name;
            let rx_name = format_ident!("{}_rx", handler_method);
            // Derive subscription method from event type
            let subscription_method = derive_subscription_method(&handler.event_type);
            quote! {
                let mut #rx_name = self.#field.#subscription_method();
            }
        })
        .collect();

    // Generate select! arms for events
    let event_select_arms: Vec<TokenStream> = event_handlers
        .iter()
        .map(|handler| {
            let method = &handler.method_name;
            let rx_name = format_ident!("{}_rx", method);
            quote! {
                Ok(event) = #rx_name.recv() => {
                    state.write().unwrap().#method(event);
                }
            }
        })
        .collect();

    // Combine command and event handling in select!
    let has_commands = !commands.is_empty();
    let has_events = !event_handlers.is_empty();

    let loop_body = if has_commands && has_events {
        quote! {
            tokio::select! {
                Some(cmd) = cmd_rx.recv() => {
                    cmd.execute(&mut *state.write().unwrap());
                }
                #(#event_select_arms)*
            }
        }
    } else if has_commands {
        quote! {
            if let Some(cmd) = cmd_rx.recv().await {
                cmd.execute(&mut *state.write().unwrap());
            }
        }
    } else if has_events {
        quote! {
            tokio::select! {
                #(#event_select_arms)*
            }
        }
    } else {
        // Neither commands nor events - shouldn't really happen, but handle gracefully
        quote! {
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        }
    };

    quote! {
        impl #struct_name {
            /// Spawns this service and returns a handle for interacting with it.
            pub fn spawn(mut self) -> #handle_name {
                let (cmd_tx, mut cmd_rx) = grove::runtime::mpsc::channel::<#command_enum_name>(256);

                // Wire up event channels (no-op if service doesn't emit events)
                self.__wire_emitter();

                // Subscribe to events BEFORE moving self into Arc
                #(#event_receiver_setup)*

                let state = grove::runtime::Arc::new(grove::runtime::RwLock::new(self));
                let state_clone = state.clone();

                tokio::spawn(async move {
                    let state = state_clone;
                    loop {
                        #loop_body
                    }
                });

                #handle_name {
                    state,
                    cmd_tx,
                }
            }
        }
    }
}
