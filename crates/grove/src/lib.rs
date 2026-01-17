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

// Re-export event types
pub use event::EventReceiver;

// Re-export metrics types
pub mod metrics;

// Cancellation token
mod cancellation;
pub use cancellation::CancellationToken;

// Spawner abstraction for executor-agnostic design
mod spawner;
pub use spawner::{SmolSpawner, Spawner, TaskHandle};

// Re-export runtime dependencies that generated code needs.
#[doc(hidden)]
pub mod runtime {
    pub use std::sync::{Arc, Mutex, RwLock};
    pub use async_channel as mpsc;
    pub use async_broadcast as broadcast;
    pub use crate::CancellationToken;
    pub use crate::{Spawner, SmolSpawner, TaskHandle};
    pub use futures;
    pub use futures::FutureExt;
    pub use async_io::Timer;

    use std::fmt;
    use std::time::Duration;

    /// Handle for waiting on task completion after shutdown.
    pub struct TaskCompletion {
        handles: Arc<Mutex<Vec<Box<dyn TaskHandle>>>>,
    }

    impl TaskCompletion {
        /// Creates a new TaskCompletion from shared JoinHandles.
        pub fn new(handles: Arc<Mutex<Vec<Box<dyn TaskHandle>>>>) -> Self {
            Self { handles }
        }

        /// Blocks until all tasks complete.
        ///
        /// Note: Task panics propagate directly and cannot be caught
        /// in the same way as with tokio. If a task panics, the panic will propagate
        /// to this call.
        pub fn wait(&self) -> Result<(), TaskPanicked> {
            let handles: Vec<_> = self.handles.lock().unwrap().drain(..).collect();
            for handle in handles {
                handle.block_on();
            }

            // If we reach here, no panics occurred
            Ok(())
        }

        /// Blocks until all tasks complete, with a timeout.
        ///
        /// Returns `Err(WaitError::Timeout)` if the timeout expires.
        /// Returns `Err(WaitError::Panicked(_))` if any task panicked.
        pub fn wait_timeout(&self, duration: Duration) -> Result<(), WaitError> {
            use futures_lite::future;

            future::block_on(async {
                let wait_future = async {
                    let handles: Vec<_> = self.handles.lock().unwrap().drain(..).collect();
                    for handle in handles {
                        handle.block_on();
                    }
                };

                let timeout_future = Timer::after(duration);

                // Race between wait and timeout
                futures_lite::future::or(
                    async {
                        wait_future.await;
                        Ok(())
                    },
                    async {
                        timeout_future.await;
                        Err(WaitError::Timeout)
                    },
                )
                .await
            })
        }

        /// Returns true if all tasks have completed.
        ///
        /// Note: We track completion by checking if handles are empty
        /// after draining. This is a simplification from the tokio implementation.
        pub fn is_complete(&self) -> bool {
            self.handles.lock().unwrap().is_empty()
        }

        /// Combines multiple TaskCompletions into one.
        ///
        /// Useful for waiting on multiple services to complete.
        ///
        /// ```ignore
        /// let completion = TaskCompletion::join([
        ///     child_handle.cancel(),
        ///     self.cancel(),
        /// ]);
        /// completion.wait()?;
        /// ```
        pub fn join<I>(completions: I) -> Self
        where
            I: IntoIterator<Item = TaskCompletion>,
        {
            let combined = Arc::new(Mutex::new(Vec::new()));
            for completion in completions {
                let mut handles = completion.handles.lock().unwrap();
                combined.lock().unwrap().append(&mut handles);
            }
            Self { handles: combined }
        }
    }

    /// Error returned when one or more tasks panicked.
    #[derive(Debug)]
    pub struct TaskPanicked {
        /// Panic messages from failed tasks.
        pub panics: Vec<String>,
    }

    impl fmt::Display for TaskPanicked {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "{} task(s) panicked", self.panics.len())
        }
    }

    impl std::error::Error for TaskPanicked {}

    /// Error returned from `wait_timeout`.
    #[derive(Debug)]
    pub enum WaitError {
        /// The timeout expired before all tasks completed.
        Timeout,
        /// One or more tasks panicked.
        Panicked(TaskPanicked),
    }

    impl fmt::Display for WaitError {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            match self {
                WaitError::Timeout => write!(f, "timeout waiting for tasks"),
                WaitError::Panicked(p) => write!(f, "{}", p),
            }
        }
    }

    impl std::error::Error for WaitError {}
}

/// Event infrastructure.
pub mod event {
    use std::any::{Any, TypeId};
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;
    use async_broadcast::{Receiver, Sender};

    /// Marker trait for event types.
    ///
    /// Implemented automatically by `#[derive(Event)]`.
    pub trait Event: Clone + Send + 'static {}

    /// A receiver for events from a Grove service.
    ///
    /// Wraps a broadcast channel receiver with a simpler API for external consumers.
    pub struct EventReceiver<T>(Receiver<T>);

    impl<T: Clone> EventReceiver<T> {
        /// Creates a new EventReceiver from a broadcast receiver.
        pub fn new(rx: Receiver<T>) -> Self {
            Self(rx)
        }

        /// Non-blocking receive. Returns `Some(event)` if available, `None` otherwise.
        ///
        /// Use this in render loops or anywhere you want to poll without blocking.
        pub fn try_recv(&mut self) -> Option<T> {
            self.0.try_recv().ok()
        }

        /// Blocking receive. Waits until an event is available or the channel closes.
        ///
        /// Returns `None` if the channel is closed.
        pub fn recv(&mut self) -> Option<T> {
            futures_lite::future::block_on(async { self.0.recv().await.ok() })
        }

        /// Returns the inner broadcast receiver.
        #[doc(hidden)]
        pub fn into_inner(self) -> Receiver<T> {
            self.0
        }

        /// Returns the number of events waiting in this receiver's queue.
        ///
        /// This is the subscriber-side queue depth - how many events have been
        /// published but not yet received by this subscriber.
        pub fn depth(&self) -> usize {
            self.0.len()
        }
    }

    /// Emitter for sending events to subscribers.
    ///
    /// Each service that emits events has an Emitter field.
    /// The macro wires up broadcast senders for each declared event type.
    #[derive(Clone)]
    pub struct Emitter {
        senders: Arc<HashMap<TypeId, Box<dyn Any + Send + Sync>>>,
        /// Published count per event type.
        counters: Arc<HashMap<TypeId, AtomicU64>>,
        /// Keep inactive receivers alive to prevent channels from closing.
        /// async_broadcast closes the channel when all receivers (including inactive) are dropped.
        _inactive_receivers: Arc<Vec<Box<dyn Any + Send + Sync>>>,
    }

    impl Emitter {
        /// Creates a new empty emitter.
        pub fn new() -> Self {
            Self {
                senders: Arc::new(HashMap::new()),
                counters: Arc::new(HashMap::new()),
                _inactive_receivers: Arc::new(Vec::new()),
            }
        }

        /// Creates an emitter with pre-registered senders.
        #[doc(hidden)]
        pub fn with_senders(
            senders: HashMap<TypeId, Box<dyn Any + Send + Sync>>,
            counters: HashMap<TypeId, AtomicU64>,
            inactive_receivers: Vec<Box<dyn Any + Send + Sync>>,
        ) -> Self {
            Self {
                senders: Arc::new(senders),
                counters: Arc::new(counters),
                _inactive_receivers: Arc::new(inactive_receivers),
            }
        }

        /// Emits an event to all subscribers.
        ///
        /// If no subscribers exist for this event type, this is a no-op.
        pub fn emit<E: Event>(&self, event: E) {
            if let Some(sender) = self.senders.get(&TypeId::of::<E>()) {
                if let Some(tx) = sender.downcast_ref::<Sender<E>>() {
                    // Ignore send errors (no receivers is fine)
                    let _ = tx.try_broadcast(event);
                    // Increment published counter
                    if let Some(counter) = self.counters.get(&TypeId::of::<E>()) {
                        counter.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
        }

        /// Subscribes to an event type.
        ///
        /// Returns a receiver that will get all future events of this type.
        /// Panics if this emitter wasn't configured to emit this event type.
        pub fn subscribe<E: Event>(&self) -> Receiver<E> {
            self.senders
                .get(&TypeId::of::<E>())
                .and_then(|sender| sender.downcast_ref::<Sender<E>>())
                .expect("event type not registered with this emitter")
                .new_receiver()
        }

        /// Returns the number of events published for this event type.
        pub fn published<E: Event>(&self) -> u64 {
            self.counters
                .get(&TypeId::of::<E>())
                .map(|c| c.load(Ordering::Relaxed))
                .unwrap_or(0)
        }

        /// Returns the current number of subscribers for this event type.
        pub fn subscriber_count<E: Event>(&self) -> usize {
            self.senders
                .get(&TypeId::of::<E>())
                .and_then(|sender| sender.downcast_ref::<Sender<E>>())
                .map(|tx| tx.receiver_count())
                .unwrap_or(0)
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
        counters: HashMap<TypeId, AtomicU64>,
        // Keep inactive receivers alive to prevent channel from closing
        _inactive_receivers: Vec<Box<dyn Any + Send + Sync>>,
    }

    impl Default for EmitterBuilder {
        fn default() -> Self {
            Self::new()
        }
    }

    impl EmitterBuilder {
        pub fn new() -> Self {
            Self {
                senders: HashMap::new(),
                counters: HashMap::new(),
                _inactive_receivers: Vec::new(),
            }
        }

        /// Adds a broadcast sender for an event type, returns the sender for subscription.
        pub fn add_event<E: Event>(&mut self, capacity: usize) -> Sender<E> {
            let (tx, rx) = async_broadcast::broadcast(capacity);
            self.senders.insert(TypeId::of::<E>(), Box::new(tx.clone()));
            self.counters.insert(TypeId::of::<E>(), AtomicU64::new(0));
            // Keep the inactive receiver alive to prevent the channel from closing
            self._inactive_receivers.push(Box::new(rx));
            tx
        }

        pub fn build(self) -> Emitter {
            Emitter::with_senders(self.senders, self.counters, self._inactive_receivers)
        }
    }
}
