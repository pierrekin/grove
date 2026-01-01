//! # Grove Events Example
//!
//! Demonstrates the event broker system:
//! - `Chat` emits `MessageSent` events
//! - `Analytics` subscribes to those events from the chat service
//!
//! The DAG: Chat -> Analytics (events flow up)
//!
//! Run with: `cargo run --example events`

use grove::Event;

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
// Analytics Service (subscribes to MessageSent from chat)
// =============================================================================

#[grove::service]
struct Analytics {
    #[grove(get)]
    message_count: usize,

    #[grove(skip)]
    chat: ChatHandle,
}

#[grove::handlers]
impl Analytics {
    // Subscribe to MessageSent events from the chat field
    #[grove(from = chat)]
    fn on_message_sent(&mut self, event: MessageSent) {
        println!(
            "📊 Analytics: User {} sent '{}'",
            event.author_id, event.content
        );
        self.message_count += 1;
    }
}

// =============================================================================
// Bootstrap
// =============================================================================

#[tokio::main]
async fn main() {
    // Spawn Chat - emitter is wired automatically!
    let chat = Chat::new(vec![]).spawn();

    // Spawn Analytics, passing the chat handle for event subscription
    let analytics = Analytics::new(0, chat.clone()).spawn();

    // Send some messages
    chat.send(Message {
        author_id: 1,
        content: "Hello from Grove!".into(),
    });

    chat.send(Message {
        author_id: 2,
        content: "Events are working!".into(),
    });

    // Give the service loops time to process
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // Check the analytics
    println!("\n📈 Analytics Stats:");
    println!("   Messages tracked: {}", analytics.message_count());
    println!("   Chat messages: {}", chat.messages().len());
}
