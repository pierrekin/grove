//! # Grove Macros
//!
//! Procedural macros for the Grove actor framework.

use proc_macro::TokenStream;
use syn::{parse_macro_input, DeriveInput};

mod event;
mod handlers;
mod service;
mod service_attr;

/// Derives the actor infrastructure for a service struct.
///
/// Generates:
/// - `{Name}Handle` struct for interacting with the service
/// - Getter methods for fields marked with `#[grove(get)]`
///
/// Optional attributes:
/// - `#[grove(emits = [Event1, Event2])]` - declares events this service can emit
///
/// # Example
///
/// ```rust,ignore
/// #[derive(Service)]
/// #[grove(emits = [MessageSent])]
/// struct Chat {
///     #[grove(get)]
///     messages: Vec<Message>,
///
///     #[grove(emitter)]
///     emit: grove::event::Emitter,
/// }
/// ```
#[proc_macro_derive(Service, attributes(grove))]
pub fn derive_service(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);

    service::expand(input)
        .unwrap_or_else(|err| err.to_compile_error())
        .into()
}

/// Derives the Event marker trait for an event struct.
///
/// Events must be Clone (for broadcast channels).
///
/// # Example
///
/// ```rust,ignore
/// #[derive(Clone, Event)]
/// struct MessageSent {
///     message: Message,
/// }
/// ```
#[proc_macro_derive(Event)]
pub fn derive_event(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);

    event::expand(input)
        .unwrap_or_else(|err| err.to_compile_error())
        .into()
}

/// Marks an impl block as containing command and event handlers.
///
/// # Example
///
/// ```rust,ignore
/// #[grove::handlers]
/// impl Chat {
///     // Command handler
///     #[grove(command)]
///     fn send(&mut self, msg: Message) {
///         self.emit_message_sent(MessageSent { message: msg.clone() });
///         self.messages.push(msg);
///     }
/// }
///
/// #[grove::handlers]
/// impl Analytics {
///     // Event handler - subscribes to events from the `chat` field
///     #[grove(from = chat)]
///     fn on_message(&mut self, event: MessageSent) {
///         self.count += 1;
///     }
/// }
/// ```
#[proc_macro_attribute]
pub fn handlers(attr: TokenStream, item: TokenStream) -> TokenStream {
    handlers::expand(attr.into(), item.into())
        .unwrap_or_else(|err| err.to_compile_error())
        .into()
}

/// Attribute macro for defining a Grove service.
///
/// This is the preferred way to define services. It automatically:
/// - Injects an emitter field for services that emit events
/// - Generates the handle struct
/// - Generates a `new()` constructor (fields with `#[grove(default)]` are excluded)
/// - Generates getter methods for `#[grove(get)]` fields
/// - Generates `emit_<event>()` methods for each declared event
/// - Generates subscription methods on the handle
///
/// # Field Attributes
///
/// - `#[grove(get)]` - Generates a getter method on the handle that clones the field
/// - `#[grove(default)]` - Excludes field from `new()`, initializes with `Default::default()`
/// - `#[grove(default = <expr>)]` - Excludes field from `new()`, initializes with the expression
///
/// Field attributes can be combined: `#[grove(get, default)]`
///
/// # Example
///
/// ```rust,ignore
/// #[grove::service]
/// #[grove(emits = [MessageSent])]
/// struct Chat {
///     #[grove(get, default)]
///     messages: Vec<Message>,
///
///     #[grove(default = 100)]
///     max_messages: usize,
/// }
///
/// #[grove::handlers]
/// impl Chat {
///     #[grove(command)]
///     fn send(&mut self, msg: Message) {
///         self.emit_message_sent(MessageSent { ... });
///         self.messages.push(msg);
///     }
/// }
///
/// // Usage: Chat::new() - no arguments needed!
/// let chat = Chat::new().spawn();
/// ```
#[proc_macro_attribute]
pub fn service(attr: TokenStream, item: TokenStream) -> TokenStream {
    service_attr::expand(attr.into(), item.into())
        .unwrap_or_else(|err| err.to_compile_error())
        .into()
}
