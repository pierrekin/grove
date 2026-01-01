//! Implementation of the `#[derive(Event)]` macro.
//!
//! This macro marks a struct as an event type that can be emitted by services
//! and subscribed to by other services.

use proc_macro2::TokenStream;
use quote::quote;
use syn::{DeriveInput, Result};

/// Main entry point for the Event derive macro.
///
/// Currently this just ensures the type implements Clone (required for broadcast).
/// Future versions may add more functionality.
pub fn expand(input: DeriveInput) -> Result<TokenStream> {
    let name = &input.ident;

    // Events must be Clone for broadcast channels
    // We generate a marker trait impl to identify event types
    Ok(quote! {
        impl grove::event::Event for #name {}
    })
}
