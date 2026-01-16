//! # Grove Hello World
//!
//! Demonstrates the Grove actor framework with two services:
//! - `NotificationService` - a leaf service with no dependencies
//! - `Chat` - depends on NotificationService, calls down the DAG
//!
//! Run with: `cargo run --example hello_world`


// =============================================================================
// Notification Service (leaf node - no dependencies)
// =============================================================================

#[derive(Clone, Debug)]
struct Notification {
    user_id: u64,
    text: String,
}

#[grove::service]
struct NotificationService {
    #[grove(get, default)]
    sent_count: usize,
}

#[grove::handlers]
impl NotificationService {
    #[grove(command)]
    fn notify(&mut self, n: Notification) {
        println!("📬 User {}: {}", n.user_id, n.text);
        self.sent_count += 1;
    }
}

// =============================================================================
// Chat Service (depends on Notification - higher in the DAG)
// =============================================================================

#[derive(Clone, Debug)]
struct Message {
    author_id: u64,
    content: String,
}

#[grove::service]
struct Chat {
    #[grove(get, default)]
    messages: Vec<Message>,

    notifications: NotificationServiceHandle,
}

#[grove::handlers]
impl Chat {
    #[grove(command)]
    fn send(&mut self, msg: Message) {
        // Call down the DAG - fire-and-forget to NotificationService
        self.notifications.notify(Notification {
            user_id: msg.author_id,
            text: format!("Message sent: {}", msg.content),
        });

        self.messages.push(msg);
    }
}

// =============================================================================
// Bootstrap
// =============================================================================

#[async_std::main]
async fn main() {
    // Spawn leaf services first (no dependencies)
    let notifications = NotificationService::new().spawn();

    // Spawn dependent services, injecting handles
    let chat = Chat::new(notifications.clone()).spawn();

    // Use the generated sender methods
    chat.send(Message {
        author_id: 1,
        content: "Hello from Grove!".into(),
    });

    chat.send(Message {
        author_id: 2,
        content: "This is an actor framework.".into(),
    });

    // Give the service loops time to process
    async_std::task::sleep(std::time::Duration::from_millis(10)).await;

    // Read state via generated getters
    println!("\n📊 Stats:");
    println!("   Messages: {}", chat.messages().len());
    println!("   Notifications sent: {}", notifications.sent_count());
}
