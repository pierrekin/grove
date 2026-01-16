//! Implementation of the `#[grove::handlers]` attribute macro.
//!
//! This macro is placed on an impl block and:
//! - Finds methods marked with `#[grove(command)]` - generates command handling
//! - Finds methods marked with `#[grove(command, poll)]` - commands that queue for poll()
//! - Finds methods marked with `#[grove(direct)]` - exposed on handle for direct calls
//! - Finds methods marked with `#[grove(from = field)]` - generates event subscriptions
//! - Generates spawn() method with select! for commands and events

use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::{
    parse::Parser, parse2, Error, FnArg, Ident, ImplItem, ImplItemFn, ItemImpl, Pat, Result, Type,
};

/// A parsed command method.
struct CommandMethod {
    method_name: Ident,
    variant_name: Ident,
    /// Parameters for the command (excludes poll args if queues_for_poll)
    params: Vec<(Ident, Type)>,
    /// If true, this command queues work for poll() instead of running directly
    queues_for_poll: bool,
}

/// A parsed direct method - exposed on handle for direct calls.
struct DirectMethod {
    method_name: Ident,
    /// All parameters including poll args
    params: Vec<(Ident, Type)>,
    /// Return type of the method
    return_type: syn::ReturnType,
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

/// A parsed background task method.
struct TaskMethod {
    /// The method name
    method_name: Ident,
    /// Extra parameters beyond handle (name, type) - empty if handle-only
    extra_params: Vec<(Ident, Type)>,
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

    // Parse impl-level attributes for poll signature
    let poll_signature = parse_poll_from_impl(&impl_block)?;
    let poll_arg_count = poll_signature.as_ref().map(|v| v.len()).unwrap_or(0);

    // Parse all methods
    let mut command_methods: Vec<CommandMethod> = Vec::new();
    let mut direct_methods: Vec<DirectMethod> = Vec::new();
    let mut direct_mut_methods: Vec<DirectMethod> = Vec::new();
    let mut event_handlers: Vec<EventHandler> = Vec::new();
    let mut task_methods: Vec<TaskMethod> = Vec::new();
    let mut clean_items: Vec<ImplItem> = Vec::new();

    for item in impl_block.items.iter() {
        if let ImplItem::Fn(method) = item {
            let attrs = parse_method_attrs(method)?;

            if attrs.is_command {
                let all_params = extract_all_params(&method.sig.inputs)?;
                // If poll flag is set, skip poll args from command params
                let params = if attrs.is_poll {
                    all_params.into_iter().skip(poll_arg_count).collect()
                } else {
                    all_params
                };

                command_methods.push(CommandMethod {
                    method_name: method.sig.ident.clone(),
                    variant_name: format_ident!("{}", to_pascal_case(&method.sig.ident.to_string())),
                    params,
                    queues_for_poll: attrs.is_poll,
                });
                clean_items.push(ImplItem::Fn(strip_grove_attrs(method.clone())));
            } else if attrs.is_direct {
                direct_methods.push(DirectMethod {
                    method_name: method.sig.ident.clone(),
                    params: extract_all_params(&method.sig.inputs)?,
                    return_type: method.sig.output.clone(),
                });
                clean_items.push(ImplItem::Fn(strip_grove_attrs(method.clone())));
            } else if attrs.is_direct_mut {
                direct_mut_methods.push(DirectMethod {
                    method_name: method.sig.ident.clone(),
                    params: extract_all_params(&method.sig.inputs)?,
                    return_type: method.sig.output.clone(),
                });
                clean_items.push(ImplItem::Fn(strip_grove_attrs(method.clone())));
            } else if let Some(source_field) = attrs.from_field {
                let (_, event_type) = extract_param(&method.sig.inputs)?
                    .ok_or_else(|| Error::new_spanned(method, "event handler must have an event parameter"))?;
                event_handlers.push(EventHandler {
                    method_name: method.sig.ident.clone(),
                    source_field,
                    event_type,
                });
                clean_items.push(ImplItem::Fn(strip_grove_attrs(method.clone())));
            } else if attrs.is_task {
                let all_params = extract_all_params(&method.sig.inputs)?;
                // Skip first two params (handle, cancel_token), keep the rest as extra params
                let extra_params = all_params.into_iter().skip(2).collect();
                task_methods.push(TaskMethod {
                    method_name: method.sig.ident.clone(),
                    extra_params,
                });
                clean_items.push(ImplItem::Fn(strip_grove_attrs(method.clone())));
            } else {
                clean_items.push(item.clone());
            }
        } else {
            clean_items.push(item.clone());
        }
    }

    // Generate command infrastructure
    let cmd_count = command_methods.len();
    let (command_enum, execute_impl, handle_sender_impl, service_queue_impl, metrics_impl) = generate_command_infra(
        &struct_name,
        &command_enum_name,
        &handle_name,
        &command_methods,
        poll_signature.as_ref(),
    );

    // Generate handle struct (must be in handlers.rs because we know command count for metrics)
    let handle_struct = quote! {
        /// Handle for interacting with the service.
        #[derive(Clone)]
        pub struct #handle_name {
            state: grove::runtime::Arc<grove::runtime::RwLock<#struct_name>>,
            cmd_tx: grove::runtime::mpsc::Sender<#command_enum_name>,
            metrics: grove::runtime::Arc<grove::metrics::CommandMetrics<#cmd_count>>,
        }
    };

    // Generate spawn implementation
    let spawn_impl = generate_spawn_impl(
        &struct_name,
        &handle_name,
        &command_enum_name,
        &command_methods,
        &event_handlers,
        &task_methods,
    );

    // Generate direct method wrappers on handle
    let direct_impl = generate_direct_methods(&struct_name, &handle_name, &direct_methods);
    let direct_mut_impl = generate_direct_mut_methods(&struct_name, &handle_name, &direct_mut_methods);

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
        #handle_struct
        #clean_impl
        #handle_sender_impl
        #spawn_impl
        #service_queue_impl
        #direct_impl
        #direct_mut_impl
        #metrics_impl
    })
}

// ============================================================================
// Parsing
// ============================================================================

/// Parsed grove attributes from a method.
struct MethodAttrs {
    is_command: bool,
    is_poll: bool,
    is_direct: bool,
    is_direct_mut: bool,
    is_task: bool,
    from_field: Option<Ident>,
}

fn parse_method_attrs(method: &ImplItemFn) -> Result<MethodAttrs> {
    let mut attrs = MethodAttrs {
        is_command: false,
        is_poll: false,
        is_direct: false,
        is_direct_mut: false,
        is_task: false,
        from_field: None,
    };

    for attr in &method.attrs {
        if !attr.path().is_ident("grove") {
            continue;
        }

        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("command") {
                attrs.is_command = true;
            } else if meta.path.is_ident("poll") {
                attrs.is_poll = true;
            } else if meta.path.is_ident("direct") {
                attrs.is_direct = true;
            } else if meta.path.is_ident("direct_mut") {
                attrs.is_direct_mut = true;
            } else if meta.path.is_ident("task") {
                attrs.is_task = true;
            } else if meta.path.is_ident("from") {
                meta.input.parse::<syn::Token![=]>()?;
                let field: Ident = meta.input.parse()?;
                attrs.from_field = Some(field);
            }
            Ok(())
        })?;
    }

    Ok(attrs)
}

/// Parse #[grove(poll(&mut T1, &mut T2))] from impl block attributes.
fn parse_poll_from_impl(impl_block: &ItemImpl) -> Result<Option<Vec<Type>>> {
    let mut poll_types: Option<Vec<Type>> = None;

    for attr in &impl_block.attrs {
        if !attr.path().is_ident("grove") {
            continue;
        }

        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("poll") {
                // Parse (&mut T1, &mut T2, ...)
                let content;
                syn::parenthesized!(content in meta.input);

                let parser =
                    syn::punctuated::Punctuated::<Type, syn::Token![,]>::parse_terminated;
                let punctuated = parser.parse2(content.parse()?)?;
                poll_types = Some(punctuated.into_iter().collect());
            }
            Ok(())
        })?;
    }

    Ok(poll_types)
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

fn extract_all_params(
    inputs: &syn::punctuated::Punctuated<FnArg, syn::token::Comma>,
) -> Result<Vec<(Ident, Type)>> {
    let mut params = Vec::new();
    for input in inputs.iter() {
        if let FnArg::Typed(pat_type) = input {
            let param_name = match &*pat_type.pat {
                Pat::Ident(pat_ident) => pat_ident.ident.clone(),
                _ => continue,
            };
            let param_type = (*pat_type.ty).clone();
            params.push((param_name, param_type));
        }
    }
    Ok(params)
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
    poll_types: Option<&Vec<Type>>,
) -> (TokenStream, TokenStream, TokenStream, TokenStream, TokenStream) {
    let cmd_count = commands.len();

    // Generate enum variants
    let enum_variants: Vec<TokenStream> = commands
        .iter()
        .map(|cmd| {
            let variant = &cmd.variant_name;
            if cmd.params.is_empty() {
                quote! { #variant, }
            } else {
                let types: Vec<_> = cmd.params.iter().map(|(_, ty)| ty).collect();
                quote! { #variant(#(#types),*), }
            }
        })
        .collect();

    // Generate index() match arms
    let index_arms: Vec<TokenStream> = commands
        .iter()
        .enumerate()
        .map(|(i, cmd)| {
            let variant = &cmd.variant_name;
            if cmd.params.is_empty() {
                quote! { Self::#variant => #i, }
            } else {
                quote! { Self::#variant(..) => #i, }
            }
        })
        .collect();

    // Generate name() match arms
    let name_arms: Vec<TokenStream> = commands
        .iter()
        .map(|cmd| {
            let variant = &cmd.variant_name;
            let name = cmd.method_name.to_string();
            if cmd.params.is_empty() {
                quote! { Self::#variant => #name, }
            } else {
                quote! { Self::#variant(..) => #name, }
            }
        })
        .collect();

    // Generate NAMES array
    let names: Vec<String> = commands.iter().map(|cmd| cmd.method_name.to_string()).collect();

    // For empty enums, use unreachable!() since they can never be instantiated
    let index_body = if commands.is_empty() {
        quote! { unreachable!("empty command enum cannot be instantiated") }
    } else {
        quote! {
            match self {
                #(#index_arms)*
            }
        }
    };

    let name_body = if commands.is_empty() {
        quote! { unreachable!("empty command enum cannot be instantiated") }
    } else {
        quote! {
            match self {
                #(#name_arms)*
            }
        }
    };

    let command_enum = quote! {
        #[allow(clippy::enum_variant_names, clippy::module_name_repetitions)]
        enum #command_enum_name {
            #(#enum_variants)*
        }

        impl #command_enum_name {
            /// Number of command variants.
            const COUNT: usize = #cmd_count;

            /// Array of command names for metrics.
            const NAMES: [&'static str; #cmd_count] = [#(#names),*];

            /// Returns the index of this command variant.
            #[inline]
            fn index(&self) -> usize {
                #index_body
            }

            /// Returns the name of this command variant.
            #[inline]
            fn name(&self) -> &'static str {
                #name_body
            }
        }
    };

    // For poll commands, we need poll type info
    let poll_types_vec = poll_types.cloned().unwrap_or_default();
    let poll_param_names: Vec<Ident> = poll_types_vec
        .iter()
        .enumerate()
        .map(|(i, _)| format_ident!("__poll_arg_{}", i))
        .collect();

    // Generate execute impl - poll commands queue work, regular commands run directly
    let match_arms: Vec<TokenStream> = commands
        .iter()
        .map(|cmd| {
            let variant = &cmd.variant_name;
            let method = &cmd.method_name;
            let param_names: Vec<_> = cmd
                .params
                .iter()
                .enumerate()
                .map(|(i, _)| format_ident!("v{}", i))
                .collect();

            if cmd.queues_for_poll {
                // Poll command: queue work for later execution via poll()
                let poll_types_only: Vec<_> = poll_types_vec.iter().collect();
                let poll_param_names_ref: Vec<&Ident> = poll_param_names.iter().collect();

                if cmd.params.is_empty() {
                    quote! {
                        #command_enum_name::#variant => {
                            let work: Box<dyn FnOnce(&#struct_name, #(#poll_types_only),*) + Send> =
                                Box::new(move |__this, #(#poll_param_names_ref),*| {
                                    __this.#method(#(#poll_param_names_ref),*);
                                });
                            state.__grove_main_thread_queue.lock().unwrap().push(work);
                        }
                    }
                } else {
                    quote! {
                        #command_enum_name::#variant(#(#param_names),*) => {
                            let work: Box<dyn FnOnce(&#struct_name, #(#poll_types_only),*) + Send> =
                                Box::new(move |__this, #(#poll_param_names_ref),*| {
                                    __this.#method(#(#poll_param_names_ref,)* #(#param_names),*);
                                });
                            state.__grove_main_thread_queue.lock().unwrap().push(work);
                        }
                    }
                }
            } else {
                // Regular command: execute directly
                if cmd.params.is_empty() {
                    quote! { #command_enum_name::#variant => state.#method(), }
                } else {
                    quote! { #command_enum_name::#variant(#(#param_names),*) => state.#method(#(#param_names),*), }
                }
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

    // Generate handle sender methods with metrics instrumentation
    let sender_methods: Vec<TokenStream> = commands
        .iter()
        .enumerate()
        .map(|(idx, cmd)| {
            let method = &cmd.method_name;
            let variant = &cmd.variant_name;
            if cmd.params.is_empty() {
                quote! {
                    pub fn #method(&self) {
                        self.metrics.inc_enqueued(#idx);
                        let _ = self.cmd_tx.try_send(#command_enum_name::#variant);
                    }
                }
            } else {
                let param_decls: Vec<TokenStream> = cmd
                    .params
                    .iter()
                    .map(|(name, ty)| quote! { #name: #ty })
                    .collect();
                let param_names: Vec<_> = cmd.params.iter().map(|(name, _)| name).collect();
                quote! {
                    pub fn #method(&self, #(#param_decls),*) {
                        self.metrics.inc_enqueued(#idx);
                        let _ = self.cmd_tx.try_send(#command_enum_name::#variant(#(#param_names),*));
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

    // Generate metrics stats methods
    let metrics_impl = quote! {
        impl #handle_name {
            /// Returns per-command statistics.
            pub fn command_stats(&self) -> impl Iterator<Item = grove::metrics::CommandStats> + '_ {
                (0..#cmd_count).map(move |i| grove::metrics::CommandStats {
                    name: #command_enum_name::NAMES[i],
                    depth: self.metrics.command_depth(i),
                    total_enqueued: self.metrics.command_enqueued(i),
                    total_processed: self.metrics.command_processed(i),
                })
            }

            /// Returns aggregate statistics with historical samples.
            pub fn aggregate_stats(&self) -> grove::metrics::AggregateStats {
                self.metrics.aggregate_stats()
            }
        }
    };

    // Generate service-side queueing methods for poll commands
    // These allow the service to queue work directly (e.g., self.queue_render())
    let poll_queue_methods: Vec<TokenStream> = commands
        .iter()
        .filter(|cmd| cmd.queues_for_poll)
        .map(|cmd| {
            let method = &cmd.method_name;
            let queue_method = format_ident!("queue_{}", method);
            let poll_types_only: Vec<_> = poll_types_vec.iter().collect();
            let poll_param_names_ref: Vec<&Ident> = poll_param_names.iter().collect();

            if cmd.params.is_empty() {
                quote! {
                    /// Queues this method to be executed via poll().
                    pub fn #queue_method(&self) {
                        let work: Box<dyn FnOnce(&#struct_name, #(#poll_types_only),*) + Send> =
                            Box::new(move |__this, #(#poll_param_names_ref),*| {
                                __this.#method(#(#poll_param_names_ref),*);
                            });
                        self.__grove_main_thread_queue.lock().unwrap().push(work);
                    }
                }
            } else {
                let param_decls: Vec<TokenStream> = cmd
                    .params
                    .iter()
                    .map(|(name, ty)| quote! { #name: #ty })
                    .collect();
                let param_names: Vec<_> = cmd.params.iter().map(|(name, _)| name).collect();
                quote! {
                    /// Queues this method to be executed via poll().
                    pub fn #queue_method(&self, #(#param_decls),*) {
                        #(let #param_names = #param_names;)*
                        let work: Box<dyn FnOnce(&#struct_name, #(#poll_types_only),*) + Send> =
                            Box::new(move |__this, #(#poll_param_names_ref),*| {
                                __this.#method(#(#poll_param_names_ref,)* #(#param_names),*);
                            });
                        self.__grove_main_thread_queue.lock().unwrap().push(work);
                    }
                }
            }
        })
        .collect();

    let service_queue_impl = if poll_queue_methods.is_empty() {
        quote! {}
    } else {
        quote! {
            impl #struct_name {
                #(#poll_queue_methods)*
            }
        }
    };

    (command_enum, execute_impl, handle_impl, service_queue_impl, metrics_impl)
}

fn generate_spawn_impl(
    struct_name: &Ident,
    handle_name: &Ident,
    command_enum_name: &Ident,
    commands: &[CommandMethod],
    event_handlers: &[EventHandler],
    tasks: &[TaskMethod],
) -> TokenStream {
    // Check if any task has extra parameters (requires builder pattern)
    let has_context_tasks = tasks.iter().any(|t| !t.extra_params.is_empty());

    // Generate event receiver setup
    let event_receiver_setup: Vec<TokenStream> = event_handlers
        .iter()
        .map(|handler| {
            let field = &handler.source_field;
            let handler_method = &handler.method_name;
            let rx_name = format_ident!("{}_rx", handler_method);
            let subscription_method = derive_subscription_method(&handler.event_type);
            quote! {
                let mut #rx_name = self.#field.#subscription_method().into_inner();
            }
        })
        .collect();

    // Generate select! arms for events (grove::runtime::futures::select! syntax)
    let event_select_arms: Vec<TokenStream> = event_handlers
        .iter()
        .map(|handler| {
            let method = &handler.method_name;
            let fut_name = format_ident!("{}_rx_fut", handler.method_name);
            quote! {
                result = #fut_name => {
                    if let Ok(event) = result {
                        state.write().unwrap().#method(event);
                    }
                    // Future will be recreated at next loop iteration
                }
            }
        })
        .collect();

    // Generate event future setup (inside loop, before select)
    // Note: futures::select! requires Unpin futures, so we pin them
    let event_fut_setup: Vec<TokenStream> = event_handlers
        .iter()
        .map(|handler| {
            let rx_name = format_ident!("{}_rx", handler.method_name);
            let fut_name = format_ident!("{}_rx_fut", handler.method_name);
            quote! {
                let mut #fut_name = std::pin::pin!(#rx_name.recv().fuse());
            }
        })
        .collect();

    // Combine command and event handling in select!
    let has_commands = !commands.is_empty();
    let has_events = !event_handlers.is_empty();

    // Command processing with metrics instrumentation
    let cmd_process_with_metrics = quote! {
        let cmd_idx = cmd.index();
        cmd.execute(&mut *state.write().unwrap());
        metrics.inc_processed(cmd_idx);

        // Sampling: check if we should record a sample
        {
            let mut s = state.write().unwrap();
            s.__grove_sampling_state.inc_command();
            if s.__grove_sampling_state.should_sample() {
                metrics.record_sample();
                s.__grove_sampling_state.reset();
            }
        }
    };

    // Drain commands without metrics (on shutdown)
    let cmd_drain = quote! {
        while let Ok(cmd) = cmd_rx.try_recv() {
            cmd.execute(&mut *state.write().unwrap());
        }
    };

    let loop_body = if has_commands && has_events {
        quote! {
            use grove::runtime::FutureExt;

            #(#event_fut_setup)*

            let mut cancel_fut = std::pin::pin!(cancel_token.cancelled().fuse());
            let mut cmd_fut = std::pin::pin!(cmd_rx.recv().fuse());

            grove::runtime::futures::select! {
                _ = cancel_fut => {
                    // Drain remaining commands before exit
                    #cmd_drain
                    break;
                }
                result = cmd_fut => {
                    match result {
                        Ok(cmd) => {
                            #cmd_process_with_metrics
                        }
                        Err(_) => break, // Channel closed
                    }
                }
                #(#event_select_arms)*
            }
        }
    } else if has_commands {
        quote! {
            use grove::runtime::FutureExt;

            let mut cancel_fut = std::pin::pin!(cancel_token.cancelled().fuse());
            let mut cmd_fut = std::pin::pin!(cmd_rx.recv().fuse());

            grove::runtime::futures::select! {
                _ = cancel_fut => {
                    // Drain remaining commands before exit
                    #cmd_drain
                    break;
                }
                result = cmd_fut => {
                    match result {
                        Ok(cmd) => {
                            #cmd_process_with_metrics
                        }
                        Err(_) => break, // Channel closed
                    }
                }
            }
        }
    } else if has_events {
        quote! {
            use grove::runtime::FutureExt;

            #(#event_fut_setup)*

            let mut cancel_fut = std::pin::pin!(cancel_token.cancelled().fuse());

            grove::runtime::futures::select! {
                _ = cancel_fut => break,
                #(#event_select_arms)*
            }
        }
    } else {
        quote! {
            use grove::runtime::FutureExt;

            let mut cancel_fut = std::pin::pin!(cancel_token.cancelled().fuse());
            let mut sleep_fut = std::pin::pin!(grove::runtime::sleep(std::time::Duration::from_secs(1)).fuse());

            grove::runtime::futures::select! {
                _ = cancel_fut => break,
                _ = sleep_fut => {}
            }
        }
    };

    let cmd_count = commands.len();

    if has_context_tasks {
        // Generate builder pattern
        generate_spawn_with_builder(
            struct_name,
            handle_name,
            command_enum_name,
            cmd_count,
            tasks,
            &event_receiver_setup,
            &loop_body,
        )
    } else {
        // Generate simple spawn (current behavior)
        generate_spawn_simple(
            struct_name,
            handle_name,
            command_enum_name,
            cmd_count,
            tasks,
            &event_receiver_setup,
            &loop_body,
        )
    }
}

/// Generate simple spawn() when all tasks are handle-only
fn generate_spawn_simple(
    struct_name: &Ident,
    handle_name: &Ident,
    command_enum_name: &Ident,
    cmd_count: usize,
    tasks: &[TaskMethod],
    event_receiver_setup: &[TokenStream],
    loop_body: &TokenStream,
) -> TokenStream {
    let task_spawns: Vec<TokenStream> = tasks
        .iter()
        .map(|task| {
            let method_name = &task.method_name;
            quote! {
                {
                    let handle = handle.clone();
                    let cancel_token = cancel_token.child_token();
                    let task_handle = grove::runtime::spawn(async move {
                        #struct_name::#method_name(handle, cancel_token).await;
                    });
                    join_handles.lock().unwrap().push(task_handle);
                }
            }
        })
        .collect();

    quote! {
        impl #struct_name {
            /// Spawns this service and returns a handle for interacting with it.
            pub fn spawn(mut self) -> #handle_name {
                let (cmd_tx, cmd_rx) = grove::runtime::mpsc::bounded::<#command_enum_name>(256);

                // Create metrics for command channel
                let metrics = grove::runtime::Arc::new(
                    grove::metrics::CommandMetrics::<#cmd_count>::new()
                );

                self.__wire_emitter();

                #(#event_receiver_setup)*

                let cancel_token = self.__grove_cancel_token.clone();
                let join_handles = self.__grove_join_handles.clone();

                let state = grove::runtime::Arc::new(grove::runtime::RwLock::new(self));
                let state_clone = state.clone();

                {
                    let join_handles = join_handles.clone();
                    let cancel_token = cancel_token.clone();
                    let metrics = metrics.clone();
                    let main_loop_handle = grove::runtime::spawn(async move {
                        let state = state_clone;
                        let mut cmd_rx = cmd_rx;
                        loop {
                            #loop_body
                        }
                    });
                    join_handles.lock().unwrap().push(main_loop_handle);
                }

                let handle = #handle_name {
                    state,
                    cmd_tx,
                    metrics,
                };

                #(#task_spawns)*

                handle
            }
        }
    }
}

/// Generate builder pattern when any task has extra parameters
fn generate_spawn_with_builder(
    struct_name: &Ident,
    handle_name: &Ident,
    command_enum_name: &Ident,
    cmd_count: usize,
    tasks: &[TaskMethod],
    event_receiver_setup: &[TokenStream],
    loop_body: &TokenStream,
) -> TokenStream {
    let builder_name = format_ident!("__{struct_name}Builder");

    // Generate builder struct fields for tasks with extra params
    let builder_fields: Vec<TokenStream> = tasks
        .iter()
        .filter(|t| !t.extra_params.is_empty())
        .map(|task| {
            let field_name = format_ident!("{}_ctx", task.method_name);
            let types: Vec<_> = task.extra_params.iter().map(|(_, ty)| ty).collect();
            quote! {
                #field_name: Option<(#(#types,)*)>
            }
        })
        .collect();


    // Generate spawn_<task> methods on the service (returns builder)
    let service_spawn_methods: Vec<TokenStream> = tasks
        .iter()
        .filter(|t| !t.extra_params.is_empty())
        .map(|task| {
            let method_name = format_ident!("spawn_{}", task.method_name);
            let field_name = format_ident!("{}_ctx", task.method_name);
            let param_decls: Vec<TokenStream> = task
                .extra_params
                .iter()
                .map(|(name, ty)| quote! { #name: #ty })
                .collect();
            let param_names: Vec<_> = task.extra_params.iter().map(|(name, _)| name).collect();

            // Initialize other context fields to None
            let other_inits: Vec<TokenStream> = tasks
                .iter()
                .filter(|t| !t.extra_params.is_empty() && t.method_name != task.method_name)
                .map(|t| {
                    let fname = format_ident!("{}_ctx", t.method_name);
                    quote! { #fname: None }
                })
                .collect();

            quote! {
                pub fn #method_name(self, #(#param_decls),*) -> #builder_name {
                    #builder_name {
                        inner: self,
                        #field_name: Some((#(#param_names,)*)),
                        #(#other_inits,)*
                    }
                }
            }
        })
        .collect();

    // Generate spawn_<task> methods on the builder (for chaining)
    let builder_spawn_methods: Vec<TokenStream> = tasks
        .iter()
        .filter(|t| !t.extra_params.is_empty())
        .map(|task| {
            let method_name = format_ident!("spawn_{}", task.method_name);
            let field_name = format_ident!("{}_ctx", task.method_name);
            let param_decls: Vec<TokenStream> = task
                .extra_params
                .iter()
                .map(|(name, ty)| quote! { #name: #ty })
                .collect();
            let param_names: Vec<_> = task.extra_params.iter().map(|(name, _)| name).collect();

            quote! {
                pub fn #method_name(mut self, #(#param_decls),*) -> Self {
                    self.#field_name = Some((#(#param_names,)*));
                    self
                }
            }
        })
        .collect();

    // Generate task spawns in builder's spawn() - handles both context and handle-only tasks
    let task_spawns: Vec<TokenStream> = tasks
        .iter()
        .map(|task| {
            let method_name = &task.method_name;
            if task.extra_params.is_empty() {
                // Handle-only task
                quote! {
                    {
                        let handle = handle.clone();
                        let cancel_token = cancel_token.child_token();
                        let task_handle = grove::runtime::spawn(async move {
                            #struct_name::#method_name(handle, cancel_token).await;
                        });
                        join_handles.lock().unwrap().push(task_handle);
                    }
                }
            } else {
                // Context task - extract from builder
                let field_name = format_ident!("{}_ctx", task.method_name);
                let param_names: Vec<_> = task
                    .extra_params
                    .iter()
                    .enumerate()
                    .map(|(i, _)| format_ident!("__ctx_{}", i))
                    .collect();

                quote! {
                    if let Some((#(#param_names,)*)) = self.#field_name {
                        let handle = handle.clone();
                        let cancel_token = cancel_token.child_token();
                        let task_handle = grove::runtime::spawn(async move {
                            #struct_name::#method_name(handle, cancel_token, #(#param_names),*).await;
                        });
                        join_handles.lock().unwrap().push(task_handle);
                    }
                }
            }
        })
        .collect();

    quote! {
        // Hidden builder struct
        #[doc(hidden)]
        pub struct #builder_name {
            inner: #struct_name,
            #(#builder_fields,)*
        }

        impl #struct_name {
            /// Internal: spawns actor without tasks (used by builder)
            fn __spawn_core(mut self) -> (#handle_name, grove::runtime::Arc<grove::runtime::Mutex<Vec<grove::runtime::JoinHandle<()>>>>) {
                let (cmd_tx, cmd_rx) = grove::runtime::mpsc::bounded::<#command_enum_name>(256);

                // Create metrics for command channel
                let metrics = grove::runtime::Arc::new(
                    grove::metrics::CommandMetrics::<#cmd_count>::new()
                );

                self.__wire_emitter();

                #(#event_receiver_setup)*

                let cancel_token = self.__grove_cancel_token.clone();
                let join_handles = self.__grove_join_handles.clone();

                let state = grove::runtime::Arc::new(grove::runtime::RwLock::new(self));
                let state_clone = state.clone();

                {
                    let join_handles = join_handles.clone();
                    let cancel_token = cancel_token.clone();
                    let metrics = metrics.clone();
                    let main_loop_handle = grove::runtime::spawn(async move {
                        let state = state_clone;
                        let mut cmd_rx = cmd_rx;
                        loop {
                            #loop_body
                        }
                    });
                    join_handles.lock().unwrap().push(main_loop_handle);
                }

                (
                    #handle_name {
                        state,
                        cmd_tx,
                        metrics,
                    },
                    join_handles,
                )
            }

            #(#service_spawn_methods)*
        }

        impl #builder_name {
            #(#builder_spawn_methods)*

            /// Spawns this service and returns a handle for interacting with it.
            pub fn spawn(self) -> #handle_name {
                let cancel_token = self.inner.__grove_cancel_token.clone();
                let (handle, join_handles) = self.inner.__spawn_core();

                #(#task_spawns)*

                handle
            }
        }
    }
}

/// Generate direct method wrappers on the handle.
/// These simply forward calls to the underlying service method.
fn generate_direct_methods(
    _struct_name: &Ident,
    handle_name: &Ident,
    methods: &[DirectMethod],
) -> TokenStream {
    if methods.is_empty() {
        return quote! {};
    }

    let handle_methods: Vec<TokenStream> = methods
        .iter()
        .map(|method| {
            let method_name = &method.method_name;
            let return_type = &method.return_type;

            let param_decls: Vec<TokenStream> = method
                .params
                .iter()
                .map(|(name, ty)| quote! { #name: #ty })
                .collect();

            let param_names: Vec<&Ident> = method.params.iter().map(|(name, _)| name).collect();

            quote! {
                /// Direct call to this method (caller provides all arguments).
                pub fn #method_name(&self, #(#param_decls),*) #return_type {
                    self.state.read().unwrap().#method_name(#(#param_names),*)
                }
            }
        })
        .collect();

    quote! {
        impl #handle_name {
            #(#handle_methods)*
        }
    }
}

/// Generate direct_mut method wrappers on the handle.
/// These forward calls with a write lock for mutable access.
fn generate_direct_mut_methods(
    _struct_name: &Ident,
    handle_name: &Ident,
    methods: &[DirectMethod],
) -> TokenStream {
    if methods.is_empty() {
        return quote! {};
    }

    let handle_methods: Vec<TokenStream> = methods
        .iter()
        .map(|method| {
            let method_name = &method.method_name;
            let return_type = &method.return_type;

            let param_decls: Vec<TokenStream> = method
                .params
                .iter()
                .map(|(name, ty)| quote! { #name: #ty })
                .collect();

            let param_names: Vec<&Ident> = method.params.iter().map(|(name, _)| name).collect();

            quote! {
                /// Direct call to this method with write lock (caller provides all arguments).
                pub fn #method_name(&self, #(#param_decls),*) #return_type {
                    self.state.write().unwrap().#method_name(#(#param_names),*)
                }
            }
        })
        .collect();

    quote! {
        impl #handle_name {
            #(#handle_methods)*
        }
    }
}
