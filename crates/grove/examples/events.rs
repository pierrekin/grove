//! # Grove Events Example
//!
//! Demonstrates the event broker system:
//! - `Chat` emits `MessageSent` events from commands
//! - `MessageGenerator` emits `MessageSent` events directly from a task
//! - `Analytics` subscribes to events from both services
//!
//! Run with: `cargo run --example events`

use grove::Event;
use std::time::Duration;

// =============================================================================
// Events
// =============================================================================

#[derive(Clone, Debug, Event)]
struct MessageSent {
    author_id: u64,
    content: String,
}

// =============================================================================
// Chat Service (emits MessageSent events)
// =============================================================================

#[derive(Clone, Debug)]
struct Message {
    author_id: u64,
    content: String,
}

#[grove::service]
#[grove(emits = [MessageSent])]
struct Chat {
    #[grove(get)]
    messages: Vec<Message>,
    // No emitter field needed - injected automatically!
}

#[grove::handlers]
impl Chat {
    #[grove(command)]
    fn send(&mut self, msg: Message) {
        // Use the generated emit method
        self.emit_message_sent(MessageSent {
            author_id: msg.author_id,
            content: msg.content.clone(),
        });

        self.messages.push(msg);
    }
}

// =============================================================================
// Message Generator (emits events directly from a task)
// =============================================================================
//
// This service demonstrates emitting events from a background task.
// The task emits directly via handle.emit_*() - no command wrapper needed.

#[grove::service]
#[grove(emits = [MessageSent])]
struct MessageGenerator {}

#[grove::handlers]
impl MessageGenerator {
    /// Background task that emits events directly via the handle.
    /// No pass-through command needed - tasks can emit via handle.emit_*()
    #[grove(task)]
    async fn generate(
        handle: MessageGeneratorHandle,
        _cancel: grove::runtime::CancellationToken,
    ) {
        let messages = ["Ping!", "Background task running", "Still here"];
        for msg in messages {
            tokio::time::sleep(Duration::from_millis(30)).await;
            // Emit directly from task - no command wrapper needed!
            handle.emit_message_sent(MessageSent {
                author_id: 0, // System
                content: msg.to_string(),
            });
        }
    }
}

// =============================================================================
// Analytics Service (subscribes to MessageSent from both services)
// =============================================================================

#[grove::service]
struct Analytics {
    #[grove(get)]
    message_count: usize,

    chat: ChatHandle,
    generator: MessageGeneratorHandle,
}

#[grove::handlers]
impl Analytics {
    // Subscribe to MessageSent events from the chat field
    #[grove(from = chat)]
    fn on_chat_message(&mut self, event: MessageSent) {
        println!(
            "📊 Analytics: User {} sent '{}'",
            event.author_id, event.content
        );
        self.message_count += 1;
    }

    // Subscribe to MessageSent events from the generator (emitted from task)
    #[grove(from = generator)]
    fn on_generator_message(&mut self, event: MessageSent) {
        println!("📊 Analytics: System broadcast '{}'", event.content);
        self.message_count += 1;
    }
}

// =============================================================================
// Bootstrap
// =============================================================================

#[tokio::main]
async fn main() {
    // Spawn Chat - emits events from commands
    let chat = Chat::new(vec![]).spawn();

    // Spawn MessageGenerator - emits events directly from a task
    let generator = MessageGenerator::new().spawn();

    // Spawn Analytics, subscribing to events from both services
    let analytics = Analytics::new(0, chat.clone(), generator.clone()).spawn();

    // Send some messages via command
    chat.send(Message {
        author_id: 1,
        content: "Hello from Grove!".into(),
    });

    chat.send(Message {
        author_id: 2,
        content: "Events are working!".into(),
    });

    // Give time for the generator task to emit its events
    tokio::time::sleep(Duration::from_millis(150)).await;

    // Check the analytics
    println!("\n📈 Analytics Stats:");
    println!("   Messages tracked: {}", analytics.message_count());
    println!("   Chat messages: {}", chat.messages().len());
}
