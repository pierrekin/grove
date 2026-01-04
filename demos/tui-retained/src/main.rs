mod counter;

use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind},
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
    ExecutableCommand,
};
use counter::{CounterService, CounterServiceHandle};
use ratatui::prelude::*;
use std::io::stdout;

fn main() -> anyhow::Result<()> {
    let rt = tokio::runtime::Runtime::new()?;
    let _guard = rt.enter();

    // Start the counter service
    let counter = CounterService::new(0, 20).spawn();

    // Trigger initial render (sends command which queues for poll)
    counter.render();

    // Set up terminal
    enable_raw_mode()?;
    stdout().execute(EnterAlternateScreen)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout()))?;

    let result = run(&mut terminal, &counter);

    // Restore terminal
    disable_raw_mode()?;
    stdout().execute(LeaveAlternateScreen)?;

    // Clean shutdown: cancel tasks and wait for completion
    counter.cancel_tasks().wait()?;

    result
}

fn run(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    counter: &CounterServiceHandle,
) -> anyhow::Result<()> {
    loop {
        // Only redraw when there's queued work (retained-mode pattern)
        // This avoids clearing the screen when nothing changed
        if counter.has_queued_work() {
            terminal.draw(|frame| {
                counter.poll(frame);
            })?;
        }

        // Handle input
        if event::poll(std::time::Duration::from_millis(16))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    match key.code {
                        KeyCode::Char('q') => break,
                        KeyCode::Char('r') => counter.reset(),
                        KeyCode::Char(' ') => counter.increment(),
                        _ => {}
                    }
                }
            }
        }
    }

    Ok(())
}
