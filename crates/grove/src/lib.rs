//! # Grove
//!
//! A macro-based actor framework for Rust.
//!
//! Grove generates the boilerplate for a hybrid actor pattern where:
//! - **Reads** go directly through `Arc<RwLock<T>>` (no channel overhead)
//! - **Writes** are serialized through an async command channel
//!
//! ## Quick Start
//!
//! ```rust,ignore
//! use grove::Service;
//!
//! #[derive(Service)]
//! struct Chat {
//!     #[grove(get)]
//!     messages: Vec<Message>,
//! }
//!
//! #[grove::commands]
//! impl Chat {
//!     #[grove(command)]
//!     fn send(&mut self, msg: Message) {
//!         self.messages.push(msg);
//!     }
//! }
//!
//! // Usage
//! let chat = Chat { messages: vec![] }.spawn();
//! chat.send(Message { content: "Hello!".into() });
//! ```

// Re-export the derive macros
pub use grove_macros::Service;

// Re-export the attribute macro for commands
pub use grove_macros::commands;

// Re-export runtime dependencies that generated code needs.
#[doc(hidden)]
pub mod runtime {
    pub use std::sync::{Arc, RwLock};
    pub use tokio::sync::mpsc;
}
