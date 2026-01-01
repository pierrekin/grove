//! # Grove
//!
//! A macro-based actor framework for Rust.
//!
//! Grove generates the boilerplate for a hybrid actor pattern where:
//! - **Reads** go directly through `Arc<RwLock<T>>` (no channel overhead)
//! - **Writes** are serialized through an async command channel
//! - **Events** flow up the service DAG via broadcast channels

// Re-export the derive macros
pub use grove_macros::Event;
pub use grove_macros::Service;

// Re-export the attribute macros
pub use grove_macros::handlers;
pub use grove_macros::service;

// Re-export runtime dependencies that generated code needs.
#[doc(hidden)]
pub mod runtime {
    pub use std::sync::{Arc, RwLock};
    pub use tokio::sync::{broadcast, mpsc};
}

/// Event infrastructure.
pub mod event {
    use std::any::{Any, TypeId};
    use std::collections::HashMap;
    use std::sync::Arc;
    use tokio::sync::broadcast;

    /// Marker trait for event types.
    ///
    /// Implemented automatically by `#[derive(Event)]`.
    pub trait Event: Clone + Send + 'static {}

    /// Emitter for sending events to subscribers.
    ///
    /// Each service that emits events has an Emitter field.
    /// The macro wires up broadcast senders for each declared event type.
    #[derive(Clone)]
    pub struct Emitter {
        senders: Arc<HashMap<TypeId, Box<dyn Any + Send + Sync>>>,
    }

    impl Emitter {
        /// Creates a new empty emitter.
        pub fn new() -> Self {
            Self {
                senders: Arc::new(HashMap::new()),
            }
        }

        /// Creates an emitter with pre-registered senders.
        #[doc(hidden)]
        pub fn with_senders(senders: HashMap<TypeId, Box<dyn Any + Send + Sync>>) -> Self {
            Self {
                senders: Arc::new(senders),
            }
        }

        /// Emits an event to all subscribers.
        ///
        /// If no subscribers exist for this event type, this is a no-op.
        pub fn emit<E: Event>(&self, event: E) {
            if let Some(sender) = self.senders.get(&TypeId::of::<E>()) {
                if let Some(tx) = sender.downcast_ref::<broadcast::Sender<E>>() {
                    // Ignore send errors (no receivers is fine)
                    let _ = tx.send(event);
                }
            }
        }

        /// Subscribes to an event type.
        ///
        /// Returns a receiver that will get all future events of this type.
        /// Panics if this emitter wasn't configured to emit this event type.
        pub fn subscribe<E: Event>(&self) -> broadcast::Receiver<E> {
            self.senders
                .get(&TypeId::of::<E>())
                .and_then(|sender| sender.downcast_ref::<broadcast::Sender<E>>())
                .expect("event type not registered with this emitter")
                .subscribe()
        }
    }

    impl Default for Emitter {
        fn default() -> Self {
            Self::new()
        }
    }

    /// Helper to build an emitter with typed senders.
    #[doc(hidden)]
    pub struct EmitterBuilder {
        senders: HashMap<TypeId, Box<dyn Any + Send + Sync>>,
    }

    impl EmitterBuilder {
        pub fn new() -> Self {
            Self {
                senders: HashMap::new(),
            }
        }

        /// Adds a broadcast sender for an event type, returns the sender for subscription.
        pub fn add_event<E: Event>(&mut self, capacity: usize) -> broadcast::Sender<E> {
            let (tx, _) = broadcast::channel(capacity);
            self.senders.insert(TypeId::of::<E>(), Box::new(tx.clone()));
            tx
        }

        pub fn build(self) -> Emitter {
            Emitter::with_senders(self.senders)
        }
    }
}
