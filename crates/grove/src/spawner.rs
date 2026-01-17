//! Spawner trait for executor-agnostic task spawning.
//!
//! This module provides the [`Spawner`] trait that allows Grove to work with
//! any async executor. The default implementation uses smol's global executor.

use std::future::Future;

/// A handle to a spawned task that can be blocked on.
///
/// This trait is implemented for task handles from various executors.
/// It provides a way to wait for task completion in a synchronous context.
pub trait TaskHandle: Send + 'static {
    /// Blocks the current thread until the task completes.
    ///
    /// Takes `Box<Self>` to allow calling on boxed trait objects.
    fn block_on(self: Box<Self>);
}

/// Trait for spawning async tasks.
///
/// Implement this trait to integrate Grove with your executor of choice.
/// The spawner is cloned for each service that needs to spawn tasks.
///
/// # Example
///
/// ```ignore
/// use grove::{Spawner, TaskHandle};
///
/// #[derive(Clone)]
/// struct MyExecutorSpawner { /* ... */ }
///
/// impl Spawner for MyExecutorSpawner {
///     type Handle = MyTaskHandle;
///
///     fn spawn<F>(&self, future: F) -> Self::Handle
///     where
///         F: Future<Output = ()> + Send + 'static,
///     {
///         // Spawn on your executor
///     }
/// }
/// ```
pub trait Spawner: Clone + Send + Sync + 'static {
    /// The type of handle returned when spawning a task.
    type Handle: TaskHandle;

    /// Spawns a future on this executor.
    ///
    /// The future must be `Send + 'static` as it may run on any thread.
    fn spawn<F>(&self, future: F) -> Self::Handle
    where
        F: Future<Output = ()> + Send + 'static;
}

/// Default spawner using smol's global executor.
///
/// This spawner uses `smol::spawn` which requires a running executor.
/// For CLI applications, you can use `smol::block_on` in main.
/// For integration with other executors, implement [`Spawner`] directly.
#[derive(Clone, Default)]
pub struct SmolSpawner;

impl Spawner for SmolSpawner {
    type Handle = smol::Task<()>;

    fn spawn<F>(&self, future: F) -> Self::Handle
    where
        F: Future<Output = ()> + Send + 'static,
    {
        smol::spawn(future)
    }
}

impl TaskHandle for smol::Task<()> {
    fn block_on(self: Box<Self>) {
        futures_lite::future::block_on(*self);
    }
}
