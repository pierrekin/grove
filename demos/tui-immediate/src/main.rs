mod logs;

use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind},
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
    ExecutableCommand,
};
use logs::{LogGeneratorConfig, LogService, LogServiceHandle};
use ratatui::prelude::*;
use std::io::stdout;

fn main() -> anyhow::Result<()> {
    // async-std executor runs automatically, no explicit runtime needed

    // Start the log service with task init context
    // The generate_logs task receives its config via spawn_generate_logs()
    let logs = LogService::new()
        .spawn_generate_logs(LogGeneratorConfig::default())
        .spawn();

    // Set up terminal
    enable_raw_mode()?;
    stdout().execute(EnterAlternateScreen)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout()))?;

    let result = run(&mut terminal, &logs);

    // Restore terminal
    disable_raw_mode()?;
    stdout().execute(LeaveAlternateScreen)?;

    // Clean shutdown: cancel service and wait for completion
    logs.cancel().wait()?;

    result
}

fn run(terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>, logs: &LogServiceHandle) -> anyhow::Result<()> {
    loop {
        terminal.draw(|frame| {
            // Render every frame (immediate-mode UI pattern)
            logs.render_logs(frame);
        })?;

        // Handle input
        if event::poll(std::time::Duration::from_millis(16))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    match key.code {
                        KeyCode::Char('q') => break,
                        KeyCode::Char(' ') => logs.trigger_event(),
                        _ => {}
                    }
                }
            }
        }
    }

    Ok(())
}
