mod counter;
mod status;

use counter::{Counter, CounterHandle};
use eframe::egui;
use status::{Status, StatusHandle};

fn main() -> eframe::Result<()> {
    // async-std executor runs automatically, no explicit runtime needed

    // Wire up services
    let counter = Counter::new().spawn();
    let status: StatusHandle = Status::new(counter.clone()).spawn();

    eframe::run_native(
        "Grove + egui",
        eframe::NativeOptions::default(),
        Box::new(|_cc| Ok(Box::new(App { counter, status }))),
    )
}

struct App {
    counter: CounterHandle,
    status: StatusHandle,
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        status::show(ctx, &self.status);
        counter::show(ctx, &self.counter);
        ctx.request_repaint();
    }
}
