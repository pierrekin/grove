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
    #[grove(get)]
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
    let counter = Counter::new(0).spawn();

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
    #[grove(get)]  // Expose getter on handle
    public_field: String,

    private_field: i32,  // Internal only
}
```

This generates:
- `MyServiceHandle` - cloneable handle for interacting with the service
- `MyService::new(...)` - constructor taking all fields
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

Call `handle.cancel_tasks()` to signal all tasks to stop and wait for completion:

```rust
let completion = handle.cancel_tasks();
completion.wait()?;  // blocks until all tasks finish
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
let service = PriceService::new(0.0)
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

### Retained Mode (reactive updates)

For UIs that only update on changes, use `#[grove(command, poll)]`:

```rust
#[grove::service]
#[grove(poll(&mut Frame))]  // Declare poll signature
pub struct Counter {
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
| `handle.poll(args)`          | Execute queued poll work                              |
| `handle.has_queued_work()`   | Check for pending poll work                           |
| `handle.cancel_tasks()`      | Signal tasks to stop, returns `TaskCompletion`        |
| `handle.task_completion()`   | Get `TaskCompletion` for waiting on tasks             |
| `handle.cancel_token()`      | Get the cancellation token for manual use             |

## Attribute Reference

### Struct Attributes

| Attribute                          | Description                               |
|------------------------------------|-------------------------------------------|
| `#[grove::service]`                | Define a service                          |
| `#[grove(emits = [E1, E2])]`       | Declare emitted event types               |
| `#[grove(poll(&mut T1, &mut T2))]` | Declare poll signature for UI integration |

### Field Attributes

| Attribute       | Description                                       |
|-----------------|---------------------------------------------------|
| `#[grove(get)]` | Generate getter on handle (field must be `Clone`) |

### Method Attributes

| Attribute                  | Description                                                   |
|----------------------------|---------------------------------------------------------------|
| `#[grove(command)]`        | Async command handler with `&mut self`                        |
| `#[grove(command, poll)]`  | Command that queues work for `poll()`                         |
| `#[grove(direct)]`         | Direct read-only method exposed on handle                     |
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

## License

MIT
