//! # Grove Metrics Example
//!
//! Demonstrates the channel metrics system:
//! - Per-command statistics (depth, enqueued, processed counts)
//! - Aggregate statistics with historical sampling
//! - Event publisher stats (published count, subscriber count)
//! - Event subscriber stats (queue depth)
//!
//! Run with: `cargo run --example metrics`

use grove::Event;
use std::time::Duration;

// =============================================================================
// Events
// =============================================================================

#[derive(Clone, Debug, Event)]
struct WorkCompleted {
    item_id: u64,
}

// =============================================================================
// Worker Service
// =============================================================================

#[grove::service]
#[grove(emits = [WorkCompleted])]
struct Worker {
    #[grove(get, default)]
    processed_count: u64,
}

#[grove::handlers]
impl Worker {
    #[grove(command)]
    fn process_item(&mut self, item_id: u64) {
        // Simulate some work
        self.processed_count += 1;
        self.emit_work_completed(WorkCompleted { item_id });
    }

    #[grove(command)]
    fn batch_process(&mut self, items: Vec<u64>) {
        for item_id in items {
            self.processed_count += 1;
            self.emit_work_completed(WorkCompleted { item_id });
        }
    }
}

// =============================================================================
// Monitor Service (subscribes to events)
// =============================================================================

#[grove::service]
struct Monitor {
    worker: WorkerHandle,

    #[grove(get, default)]
    events_received: u64,
}

#[grove::handlers]
impl Monitor {
    #[grove(from = worker)]
    fn on_work_completed(&mut self, _event: WorkCompleted) {
        self.events_received += 1;
    }
}

// =============================================================================
// Main - Demonstrate Metrics
// =============================================================================

#[async_std::main]
async fn main() {
    println!("=== Grove Metrics Demo ===\n");

    // Spawn services
    let worker = Worker::new().spawn();
    let monitor = Monitor::new(worker.clone()).spawn();

    // Also get an external event receiver
    let mut event_rx = worker.on_work_completed();

    // ---------------------------------------------------------------------
    // 1. Command Channel Metrics
    // ---------------------------------------------------------------------
    println!("--- Command Channel Metrics ---\n");

    // Send some commands
    for i in 0..5 {
        worker.process_item(i);
    }
    worker.batch_process(vec![100, 101, 102]);

    // Give time for commands to process
    async_std::task::sleep(Duration::from_millis(50)).await;

    // Per-command statistics
    println!("Per-command stats:");
    for stat in worker.command_stats() {
        println!(
            "  {}: depth={}, enqueued={}, processed={}",
            stat.name, stat.depth, stat.total_enqueued, stat.total_processed
        );
    }

    // Aggregate statistics (includes historical sampling)
    let agg = worker.aggregate_stats();
    println!("\nAggregate stats:");
    println!("  Current depth: {}", agg.depth);
    println!("  Total enqueued: {}", agg.total_enqueued);
    println!("  Total processed: {}", agg.total_processed);
    println!("  Mean depth (1s): {:.2}", agg.mean_depth_1s);
    println!("  Max depth (1m): {}", agg.max_depth_1m);

    // ---------------------------------------------------------------------
    // 2. Event Publisher Metrics
    // ---------------------------------------------------------------------
    println!("\n--- Event Publisher Metrics ---\n");

    // Publisher-side stats (on the worker handle)
    println!("WorkCompleted event stats:");
    println!("  Published: {}", worker.work_completed_published());
    println!("  Subscriber count: {}", worker.work_completed_subscriber_count());

    // ---------------------------------------------------------------------
    // 3. Event Subscriber Metrics
    // ---------------------------------------------------------------------
    println!("\n--- Event Subscriber Metrics ---\n");

    // Send more events to build up the queue
    for i in 200..210 {
        worker.process_item(i);
    }

    // Small delay to let events be published (but not consumed from event_rx)
    async_std::task::sleep(Duration::from_millis(50)).await;

    // Subscriber-side stats (on the EventReceiver)
    println!("External receiver queue depth: {}", event_rx.depth());

    // Drain the receiver
    while event_rx.try_recv().is_some() {}
    println!("After draining: {}", event_rx.depth());

    // ---------------------------------------------------------------------
    // 4. Load test to see metrics under pressure
    // ---------------------------------------------------------------------
    println!("\n--- Load Test ---\n");

    // Burst of commands
    let burst_size = 1000;
    println!("Sending {} commands in burst...", burst_size);

    for i in 0..burst_size {
        worker.process_item(i);
    }

    // Check depth immediately (before processing completes)
    let agg = worker.aggregate_stats();
    println!("Depth right after burst: {}", agg.depth);

    // Wait for processing
    async_std::task::sleep(Duration::from_millis(200)).await;

    let agg = worker.aggregate_stats();
    println!("Depth after processing: {}", agg.depth);
    println!("Total processed: {}", agg.total_processed);

    // Final stats
    println!("\n--- Final Stats ---\n");
    println!("Worker processed count: {}", worker.processed_count());
    println!("Monitor events received: {}", monitor.events_received());
    println!("Total events published: {}", worker.work_completed_published());
}
