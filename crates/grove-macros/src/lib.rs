//! # Grove Macros
//!
//! Procedural macros for the Grove actor framework.
//!
//! This crate is not meant to be used directly. Instead, use the `grove` crate
//! which re-exports these macros.
//!
//! ## How Procedural Macros Work
//!
//! A procedural macro is a function that:
//! 1. Receives a stream of tokens (the code the user wrote)
//! 2. Parses those tokens into a structured AST
//! 3. Generates new tokens (the code we want to produce)
//! 4. Returns those tokens to the compiler
//!
//! The compiler then compiles the generated code as if the user wrote it.

use proc_macro::TokenStream;
use syn::{parse_macro_input, DeriveInput};

mod commands;
mod service;

/// Derives the actor infrastructure for a service struct.
///
/// This macro generates:
/// - A `{Name}Handle` struct for interacting with the service
/// - A `{Name}::spawn(self)` method to create and start the service
/// - Getter methods for fields marked with `#[grove(get)]`
///
/// The command enum is auto-derived as `{Name}Command`.
///
/// # Example
///
/// ```rust,ignore
/// use grove::Service;
///
/// #[derive(Service)]
/// struct Chat {
///     #[grove(get)]
///     messages: Vec<Message>,
///
///     #[grove(skip)]
///     notifications: NotificationServiceHandle,
/// }
///
/// #[grove::commands]
/// impl Chat {
///     #[grove(command)]
///     fn send_message(&mut self, msg: Message) {
///         self.messages.push(msg);
///     }
/// }
/// ```
#[proc_macro_derive(Service, attributes(grove))]
pub fn derive_service(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);

    service::expand(input)
        .unwrap_or_else(|err| err.to_compile_error())
        .into()
}

/// Marks an impl block as containing command methods.
///
/// Methods marked with `#[grove(command)]` become commands that can be
/// sent to the service through its handle.
///
/// # Example
///
/// ```rust,ignore
/// #[grove::commands]
/// impl Chat {
///     #[grove(command)]
///     fn send_message(&mut self, msg: Message) {
///         self.messages.push(msg);
///     }
///
///     #[grove(command)]
///     fn clear(&mut self) {
///         self.messages.clear();
///     }
///
///     // Regular method - not a command
///     fn helper(&self) -> usize {
///         self.messages.len()
///     }
/// }
/// ```
///
/// This generates:
/// - `enum ChatCommand { SendMessage(Message), Clear }`
/// - `impl ChatCommand { fn execute(self, state: &mut Chat) { ... } }`
/// - `impl ChatHandle { fn send_message(&self, msg: Message) { ... } }`
#[proc_macro_attribute]
pub fn commands(attr: TokenStream, item: TokenStream) -> TokenStream {
    commands::expand(attr.into(), item.into())
        .unwrap_or_else(|err| err.to_compile_error())
        .into()
}
