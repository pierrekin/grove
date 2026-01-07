# Grove

A macro-based actor framework for Rust that generates boilerplate for a hybrid actor pattern where reads are lock-free through `Arc<RwLock<T>>` and writes are serialized through async command channels.

## Features

- **Declarative service definition** with `#[grove::service]`
- **Async command handlers** for serialized state mutations
- **Event system** with typed publish/subscribe
- **Direct read access** through generated handles
- **Background tasks** that run alongside services
- **Main-thread dispatch** for UI frameworks (immediate and retained modes)

## Quick Start

```rust
use grove::Event;

#[derive(Clone, Event)]
pub struct CounterChanged {
    pub value: usize,
}

#[grove::service]
#[grove(emits = [CounterChanged])]
pub struct Counter {
    #[grove(get, default)]
    value: usize,
}

#[grove::handlers]
impl Counter {
    #[grove(command)]
    fn increment(&mut self) {
        self.value += 1;
        self.emit_counter_changed(CounterChanged { value: self.value });
    }
}

#[tokio::main]
async fn main() {
    let counter = Counter::new().spawn();

    // Send commands (async, serialized)
    counter.increment();

    // Read state (direct, lock-free reads)
    println!("Value: {}", counter.value());

    // Subscribe to events
    let mut rx = counter.on_counter_changed();
    while let Ok(event) = rx.recv().await {
        println!("Counter changed to {}", event.value);
    }
}
```

## Service Definition

Use `#[grove::service]` on a struct to define a service:

```rust
#[grove::service]
#[grove(emits = [Event1, Event2])]  // Optional: declare emitted events
pub struct MyService {
    #[grove(get, default)]  // Getter on handle + default init
    public_field: Vec<String>,

    #[grove(default = 100)]  // Custom default value
    max_items: usize,

    dependency: OtherServiceHandle,  // Required field (no default)
}
```

This generates:
- `MyServiceHandle` - cloneable handle for interacting with the service
- `MyService::new(dependency)` - constructor taking only non-defaulted fields
- `handle.public_field()` - getter for fields marked with `#[grove(get)]`
- `handle.on_event1()` - subscription methods for declared events
- `self.emit_event1(...)` - emit methods on the service (for commands)
- `handle.emit_event1(...)` - emit methods on the handle (for tasks)

## Command Handlers

Define methods in an impl block with `#[grove::handlers]`:

```rust
#[grove::handlers]
impl MyService {
    #[grove(command)]
    fn do_something(&mut self, arg: String) {
        self.private_field += 1;
        // Commands have exclusive mutable access
    }
}
```

Commands are:
- Sent through an async channel
- Executed serially on the service's background task
- Have `&mut self` access for state mutations

## Events

Define events with the `Event` derive macro:

```rust
#[derive(Clone, Event)]
pub struct MessageSent {
    pub content: String,
    pub timestamp: u64,
}
```

Events must implement `Clone + Send + 'static`. Event channels have a fixed capacity of 256 messages.

Emit events from commands:

```rust
#[grove(command)]
fn send_message(&mut self, content: String) {
    self.emit_message_sent(MessageSent {
        content,
        timestamp: now(),
    });
}
```

Subscribe from other services:

```rust
#[grove::service]
pub struct Logger {
    chat: ChatHandle,  // Handle to another service
}

#[grove::handlers]
impl Logger {
    #[grove(from = chat)]  // Subscribe to chat's events
    fn on_message_sent(&mut self, event: MessageSent) {
        println!("Message: {}", event.content);
    }
}
```

## Background Tasks

Spawn long-running async tasks with your service:

```rust
use grove::runtime::CancellationToken;

#[grove::handlers]
impl PriceService {
    #[grove(task)]
    async fn poll_prices(handle: PriceServiceHandle, cancel: CancellationToken) {
        let mut interval = tokio::time::interval(Duration::from_secs(60));
        loop {
            tokio::select! {
                _ = cancel.cancelled() => break,
                _ = interval.tick() => {
                    let price = fetch_price().await;
                    handle.update_price(price);
                }
            }
        }
    }

    #[grove(command)]
    fn update_price(&mut self, price: f64) {
        self.current_price = price;
    }
}
```

Tasks receive:
1. A clone of the handle
2. A `CancellationToken` for graceful shutdown

Call `handle.cancel()` to cancel the service and wait for completion:

```rust
let completion = handle.cancel();
completion.wait()?;  // blocks until service and all tasks finish
```

On shutdown, any commands still queued in the channel are drained and executed before the service task exits. This ensures graceful shutdown without losing pending work.

### TaskCompletion

The `TaskCompletion` handle returned by `cancel()` provides several methods:

```rust
// Block until complete
completion.wait()?;

// Block with timeout
use std::time::Duration;
completion.wait_timeout(Duration::from_secs(5))?;

// Non-blocking check
if completion.is_complete() {
    println!("All tasks finished");
}

// Combine multiple services
let completion = TaskCompletion::join([
    service_a.cancel(),
    service_b.cancel(),
]);
completion.wait()?;
```

Tasks can also emit events directly via the handle:

```rust
#[grove(task)]
async fn poll_prices(handle: PriceServiceHandle, cancel: CancellationToken) {
    loop {
        tokio::select! {
            _ = cancel.cancelled() => break,
            _ = tokio::time::sleep(Duration::from_secs(60)) => {
                let price = fetch_price().await;
                handle.emit_price_updated(PriceUpdated { price });
            }
        }
    }
}
```

### Task Init Context

Tasks can receive additional parameters beyond handle and cancel token. These are passed at spawn time via generated builder methods:

```rust
pub struct PollConfig {
    pub interval: Duration,
    pub endpoint: String,
}

#[grove::handlers]
impl PriceService {
    #[grove(task)]
    async fn poll_prices(
        handle: PriceServiceHandle,
        cancel: CancellationToken,
        config: PollConfig,
    ) {
        let mut interval = tokio::time::interval(config.interval);
        loop {
            tokio::select! {
                _ = cancel.cancelled() => break,
                _ = interval.tick() => {
                    let price = fetch_price(&config.endpoint).await;
                    handle.update_price(price);
                }
            }
        }
    }
}

// Spawn with config
let service = PriceService::new()
    .spawn_poll_prices(PollConfig {
        interval: Duration::from_secs(60),
        endpoint: "https://api.example.com".into(),
    })
    .spawn();
```

When a task has extra parameters:
- `spawn_<task_name>(args)` is generated on the service
- `spawn()` is only available after calling the spawn method for each context task
- Multiple tasks can be chained: `.spawn_task_a(args).spawn_task_b(args).spawn()`

## UI Integration

Grove supports two patterns for UI frameworks:

### Immediate Mode (egui, ratatui)

For UIs that redraw every frame, use `#[grove(direct)]`:

```rust
#[grove::handlers]
impl AppState {
    #[grove(direct)]
    fn render(&self, frame: &mut Frame) {
        // Called directly with read access
        // No queuing, synchronous execution
    }
}

// Main loop
loop {
    terminal.draw(|frame| {
        app.render(frame);  // Direct call every frame
    })?;
}
```

For methods that need mutable access (e.g., during shutdown), use `#[grove(direct_mut)]`:

```rust
#[grove(direct_mut)]
fn shutdown(&mut self) {
    self.cleanup();  // Synchronous write access
}
```

### Retained Mode (reactive updates)

For UIs that only update on changes, use `#[grove(command, poll)]`:

```rust
#[grove::service]
#[grove(poll(&mut Frame))]  // Declare poll signature
pub struct Counter {
    #[grove(default)]
    value: u32,
}

#[grove::handlers]
#[grove(poll(&mut Frame))]  // Must match service declaration
impl Counter {
    #[grove(command)]
    fn increment(&mut self) {
        self.value += 1;
        self.queue_render();  // Queue a render update
    }

    #[grove(command, poll)]
    fn render(&self, frame: &mut Frame) {
        // Queued for execution via poll()
    }
}

// Main loop - only redraws when needed
loop {
    if counter.has_queued_work() {
        terminal.draw(|frame| {
            counter.poll(frame);  // Execute queued renders
        })?;
    }
}
```

The `poll` pattern:
- `#[grove(command, poll)]` methods queue work instead of executing immediately
- `self.queue_<method>()` is generated for internal queueing
- `handle.has_queued_work()` checks for pending work
- `handle.poll(args)` executes all queued work

## Handle API

Every service generates a `{Service}Handle` with:

| Method                       | Description                                           |
|------------------------------|-------------------------------------------------------|
| `handle.command(args)`       | Send a command (async, queued)                        |
| `handle.field()`             | Read a `#[grove(get)]` field (sync, cloned)           |
| `handle.on_event()`          | Subscribe to an event (returns `broadcast::Receiver`) |
| `handle.emit_event(e)`       | Emit an event (useful from tasks)                     |
| `handle.direct_method(args)` | Call a `#[grove(direct)]` method (sync, read access)  |
| `handle.direct_mut_method(args)` | Call a `#[grove(direct_mut)]` method (sync, write access) |
| `handle.poll(args)`          | Execute queued poll work                              |
| `handle.has_queued_work()`   | Check for pending poll work                           |
| `handle.cancel()`            | Cancel service and tasks, returns `TaskCompletion`    |
| `handle.task_completion()`   | Get `TaskCompletion` for waiting on tasks             |
| `handle.cancel_token()`      | Get the cancellation token for manual use             |
| `handle.command_stats()`     | Iterator of per-command metrics                       |
| `handle.aggregate_stats()`   | Aggregate metrics with historical sampling            |
| `handle.<event>_published()` | Count of events published (for emitting services)     |
| `handle.<event>_subscriber_count()` | Current subscriber count for event type        |

## Metrics

Grove automatically collects channel metrics for observability. No configuration required.

### Command Channel Metrics

Every service tracks per-command statistics:

```rust
// Per-command breakdown
for stat in handle.command_stats() {
    println!("{}: depth={}, enqueued={}, processed={}",
        stat.name,
        stat.depth,           // Current queue depth
        stat.total_enqueued,  // Total commands sent
        stat.total_processed  // Total commands processed
    );
}

// Aggregate statistics with historical sampling
let agg = handle.aggregate_stats();
println!("Current depth: {}", agg.depth);
println!("Mean depth (1s): {:.2}", agg.mean_depth_1s);
println!("Max depth (1m): {}", agg.max_depth_1m);
```

| Field | Description |
|-------|-------------|
| `depth` | Current queue depth (exact) |
| `total_enqueued` | Total commands sent |
| `total_processed` | Total commands processed |
| `mean_depth_1s/10s/1m` | Average depth over time window |
| `max_depth_1s/10s/1m` | Maximum depth over time window |
| `throughput_1s` | Approximate commands/sec |

Sampling occurs every 100 commands or 100ms (whichever comes first), with 1 minute of history retained.

### Event Metrics

For services that emit events, Grove tracks publisher-side statistics:

```rust
// Publisher stats (per event type)
let published = handle.my_event_published();      // Total events emitted
let subscribers = handle.my_event_subscriber_count();  // Current subscriber count
```

For event subscribers, check receiver queue depth:

```rust
let mut receiver = handle.on_my_event();
let pending = receiver.depth();  // Events waiting to be consumed
```

| Location | Method | Description |
|----------|--------|-------------|
| Handle | `<event>_published()` | Total events emitted |
| Handle | `<event>_subscriber_count()` | Current subscriber count |
| EventReceiver | `depth()` | Pending events in this receiver |

### Metrics API Summary

| Method | Description |
|--------|-------------|
| `handle.command_stats()` | Iterator of per-command `CommandStats` |
| `handle.aggregate_stats()` | `AggregateStats` with current + historical data |
| `handle.<event>_published()` | Total events published for this type |
| `handle.<event>_subscriber_count()` | Number of active subscribers |
| `receiver.depth()` | Events pending in this subscriber's queue |

## Attribute Reference

### Struct Attributes

| Attribute                          | Description                               |
|------------------------------------|-------------------------------------------|
| `#[grove::service]`                | Define a service                          |
| `#[grove(emits = [E1, E2])]`       | Declare emitted event types               |
| `#[grove(poll(&mut T1, &mut T2))]` | Declare poll signature for UI integration |

### Field Attributes

| Attribute                   | Description                                                |
|-----------------------------|-----------------------------------------------------------|
| `#[grove(get)]`             | Generate getter on handle (field must be `Clone`)          |
| `#[grove(default)]`         | Exclude from `new()`, init with `Default::default()`       |
| `#[grove(default = <expr>)]`| Exclude from `new()`, init with the given expression       |

Field attributes can be combined: `#[grove(get, default)]`

### Method Attributes

| Attribute                  | Description                                                   |
|----------------------------|---------------------------------------------------------------|
| `#[grove(command)]`        | Async command handler with `&mut self`                        |
| `#[grove(command, poll)]`  | Command that queues work for `poll()`                         |
| `#[grove(direct)]`         | Direct read-only method exposed on handle                     |
| `#[grove(direct_mut)]`     | Direct mutable method exposed on handle (acquires write lock) |
| `#[grove(from = field)]`   | Event handler subscribing to another service                  |
| `#[grove(task)]`           | Background async task; receives `(handle, cancel_token, ...extra_params)` |

## Demos

See the `demos/` directory for full applications:

- **egui-counter** - Event-driven counter with egui UI
- **tui-immediate** - Immediate-mode rendering with ratatui, demonstrates task init context
- **tui-retained** - Retained-mode rendering with poll queue

For smaller examples, see `crates/grove/examples/`:

- **hello_world** - Basic service, commands, and getters
- **events** - Event emission and subscription
- **metrics** - Channel metrics and observability APIs

## License

MIT
