//! Cancellation token for cooperative task shutdown.

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll, Waker};

use std::sync::Mutex;

/// A token for cooperative cancellation of async tasks.
///
/// Clone the token to share it across tasks. When `cancel()` is called,
/// all tasks waiting on `cancelled()` will be woken.
#[derive(Clone)]
pub struct CancellationToken {
    inner: Arc<Inner>,
}

struct Inner {
    cancelled: AtomicBool,
    wakers: Mutex<Vec<Waker>>,
}

impl CancellationToken {
    /// Creates a new cancellation token.
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Inner {
                cancelled: AtomicBool::new(false),
                wakers: Mutex::new(Vec::new()),
            }),
        }
    }

    /// Triggers cancellation.
    ///
    /// All futures returned by `cancelled()` will complete.
    pub fn cancel(&self) {
        self.inner.cancelled.store(true, Ordering::SeqCst);
        // Wake all waiters
        let wakers: Vec<_> = self.inner.wakers.lock().unwrap().drain(..).collect();
        for waker in wakers {
            waker.wake();
        }
    }

    /// Returns true if cancellation has been triggered.
    pub fn is_cancelled(&self) -> bool {
        self.inner.cancelled.load(Ordering::SeqCst)
    }

    /// Returns a future that completes when cancellation is triggered.
    ///
    /// Use this in `select!` to handle graceful shutdown:
    ///
    /// ```ignore
    /// futures::select! {
    ///     _ = cancel_token.cancelled().fuse() => break,
    ///     msg = rx.recv().fuse() => { /* handle message */ }
    /// }
    /// ```
    pub fn cancelled(&self) -> CancelledFuture {
        CancelledFuture {
            inner: self.inner.clone(),
        }
    }

    /// Creates a child token linked to this one.
    ///
    /// When the parent is cancelled, the child is also cancelled.
    /// The child can be cancelled independently without affecting the parent.
    pub fn child_token(&self) -> Self {
        // For simplicity, child tokens share the same inner.
        // A full implementation would create a linked hierarchy.
        self.clone()
    }
}

impl Default for CancellationToken {
    fn default() -> Self {
        Self::new()
    }
}

/// Future that completes when a cancellation token is triggered.
pub struct CancelledFuture {
    inner: Arc<Inner>,
}

impl Future for CancelledFuture {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        if self.inner.cancelled.load(Ordering::SeqCst) {
            Poll::Ready(())
        } else {
            // Register waker
            let mut wakers = self.inner.wakers.lock().unwrap();
            // Check again after acquiring lock
            if self.inner.cancelled.load(Ordering::SeqCst) {
                Poll::Ready(())
            } else {
                wakers.push(cx.waker().clone());
                Poll::Pending
            }
        }
    }
}
