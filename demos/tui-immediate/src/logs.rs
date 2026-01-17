use ratatui::{
    prelude::*,
    widgets::{Block, Borders, List, ListItem, Paragraph},
};
use std::time::{Duration, Instant};

// =============================================================================
// Service - Immediate Mode Pattern using #[grove(direct)]
// =============================================================================
//
// This example demonstrates the "direct" pattern for immediate-mode UIs.
// The main loop calls render_logs() directly every frame, and the service
// just manages state. This is the natural pattern for ratatui/egui.
//
// It also demonstrates task init context - the generate_logs task receives
// configuration at spawn time via the builder pattern.

#[grove::service]
pub struct LogService {
    #[grove(default)]
    entries: Vec<LogEntry>,

    #[grove(default = Instant::now())]
    start_time: Instant,
}

#[derive(Clone)]
pub struct LogEntry {
    timestamp: f64,
    level: LogLevel,
    message: String,
}

#[derive(Clone, Copy)]
pub enum LogLevel {
    Info,
    Warn,
    Error,
}

/// Configuration for the log generator task - passed at spawn time
pub struct LogGeneratorConfig {
    pub interval: Duration,
    pub messages: Vec<(LogLevel, &'static str)>,
}

impl Default for LogGeneratorConfig {
    fn default() -> Self {
        Self {
            interval: Duration::from_secs(2),
            messages: vec![
                (LogLevel::Info, "System heartbeat"),
                (LogLevel::Info, "Connection pool healthy"),
                (LogLevel::Warn, "Memory usage above 70%"),
                (LogLevel::Info, "Cache hit ratio: 94%"),
                (LogLevel::Error, "Failed to reach backup server"),
                (LogLevel::Info, "Retry succeeded"),
            ],
        }
    }
}

#[grove::handlers]
impl LogService {
    /// Background task that generates periodic log entries.
    ///
    /// This task demonstrates task init context - it receives configuration
    /// at spawn time via `spawn_generate_logs(config)`.
    #[grove(task)]
    async fn generate_logs(
        handle: LogServiceHandle,
        cancel: grove::runtime::CancellationToken,
        config: LogGeneratorConfig,
    ) {
        use futures::FutureExt;

        let mut idx = 0;

        loop {
            let mut cancel_fut = std::pin::pin!(cancel.cancelled().fuse());
            let mut sleep_fut = std::pin::pin!(async_io::Timer::after(config.interval).fuse());

            futures::select! {
                _ = cancel_fut => break,
                _ = sleep_fut => {
                    let (level, msg) = config.messages[idx % config.messages.len()];
                    handle.add_entry(level, msg.to_string());
                    idx += 1;
                }
            }
        }
    }

    /// Command to add a log entry
    #[grove(command)]
    fn add_entry(&mut self, level: LogLevel, message: String) {
        let timestamp = self.start_time.elapsed().as_secs_f64();
        self.entries.push(LogEntry {
            timestamp,
            level,
            message,
        });

        // Keep last 100 entries
        if self.entries.len() > 100 {
            self.entries.remove(0);
        }
    }

    /// Command to manually trigger a log event
    #[grove(command)]
    fn trigger_event(&mut self) {
        self.add_entry(LogLevel::Info, "Manual event triggered by user".to_string());
    }

    /// Renders the log view - exposed on handle for direct calls
    #[grove(direct)]
    fn render_logs(&self, frame: &mut Frame) {
        let area = frame.area();

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),
                Constraint::Min(0),
                Constraint::Length(3),
            ])
            .split(area);

        // Header
        let header = Paragraph::new("TUI Log Viewer (Direct Pattern)")
            .style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))
            .block(Block::default().borders(Borders::ALL));
        frame.render_widget(header, chunks[0]);

        // Log entries
        let items: Vec<ListItem> = self
            .entries
            .iter()
            .map(|entry| {
                let (style, prefix) = match entry.level {
                    LogLevel::Info => (Style::default().fg(Color::Green), "INFO "),
                    LogLevel::Warn => (Style::default().fg(Color::Yellow), "WARN "),
                    LogLevel::Error => (Style::default().fg(Color::Red), "ERROR"),
                };
                let content = format!("[{:>8.2}s] {} {}", entry.timestamp, prefix, entry.message);
                ListItem::new(content).style(style)
            })
            .collect();

        let logs = List::new(items)
            .block(Block::default().title("Logs").borders(Borders::ALL));
        frame.render_widget(logs, chunks[1]);

        // Footer
        let footer = Paragraph::new("Press 'q' to quit, SPACE to trigger event")
            .style(Style::default().fg(Color::DarkGray))
            .block(Block::default().borders(Borders::ALL));
        frame.render_widget(footer, chunks[2]);
    }
}
