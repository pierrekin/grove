use ratatui::{
    prelude::*,
    widgets::{Block, Borders, Gauge, Paragraph},
};
use std::sync::atomic::{AtomicU32, Ordering};

// =============================================================================
// Service - Retained Mode Pattern using #[grove(command, poll)]
// =============================================================================
//
// This example demonstrates the "command, poll" pattern for retained-mode UIs.
// The service queues render updates when state changes, and the main loop calls
// poll() to execute any queued work. Renders only happen when triggered.
//
// This pattern is useful for:
// - Retained-mode UI frameworks
// - Reducing unnecessary redraws
// - Event-driven rendering

// Track render count to demonstrate reactive rendering
static RENDER_COUNT: AtomicU32 = AtomicU32::new(0);

#[grove::service]
#[grove(poll(&mut Frame))]
pub struct CounterService {
    #[grove(default)]
    count: u32,

    #[grove(default = 20)]
    max_count: u32,
}

#[grove::handlers]
#[grove(poll(&mut Frame))]
impl CounterService {
    /// Background task that increments the counter periodically
    #[grove(task)]
    async fn tick_counter(
        handle: CounterServiceHandle,
        cancel: grove::runtime::CancellationToken,
    ) {
        let mut interval = tokio::time::interval(std::time::Duration::from_millis(500));

        loop {
            tokio::select! {
                _ = cancel.cancelled() => break,
                _ = interval.tick() => handle.increment(),
            }
        }
    }

    /// Command to increment the counter - triggers a render when done
    #[grove(command)]
    fn increment(&mut self) {
        self.count = (self.count + 1) % (self.max_count + 1);
        // Queue a render update - this will run when poll() is called
        self.queue_render();
    }

    /// Command to reset the counter
    #[grove(command)]
    fn reset(&mut self) {
        self.count = 0;
        self.queue_render();
    }

    /// Queues a render update to be executed via poll()
    /// This is a poll command - it queues work instead of running directly
    #[grove(command, poll)]
    fn render(&self, frame: &mut Frame) {
        let render_num = RENDER_COUNT.fetch_add(1, Ordering::Relaxed) + 1;

        let area = frame.area();
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),
                Constraint::Length(3),
                Constraint::Length(3),
                Constraint::Min(0),
                Constraint::Length(3),
            ])
            .split(area);

        // Header
        let header = Paragraph::new("Counter (Retained-Mode Pattern)")
            .style(Style::default().fg(Color::Magenta).add_modifier(Modifier::BOLD))
            .block(Block::default().borders(Borders::ALL));
        frame.render_widget(header, chunks[0]);

        // Counter display
        let counter_text = format!("Count: {} / {}", self.count, self.max_count);
        let counter = Paragraph::new(counter_text)
            .style(Style::default().fg(Color::Cyan))
            .block(Block::default().title("Value").borders(Borders::ALL));
        frame.render_widget(counter, chunks[1]);

        // Progress gauge
        let progress = self.count as f64 / self.max_count as f64;
        let gauge = Gauge::default()
            .block(Block::default().title("Progress").borders(Borders::ALL))
            .gauge_style(Style::default().fg(Color::Green))
            .ratio(progress);
        frame.render_widget(gauge, chunks[2]);

        // Render count - shows reactive rendering (only when state changes)
        let render_info = Paragraph::new(format!(
            "Render #{} - Only renders when state changes!",
            render_num
        ))
        .style(Style::default().fg(Color::Yellow))
        .block(Block::default().title("Reactive Rendering").borders(Borders::ALL));
        frame.render_widget(render_info, chunks[3]);

        // Footer
        let footer = Paragraph::new("Press 'q' to quit, 'r' to reset, SPACE to increment")
            .style(Style::default().fg(Color::DarkGray))
            .block(Block::default().borders(Borders::ALL));
        frame.render_widget(footer, chunks[4]);
    }
}
