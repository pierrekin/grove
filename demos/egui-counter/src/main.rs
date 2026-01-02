mod counter;
mod status;

use counter::{Counter, CounterHandle};
use eframe::egui;
use status::{Status, StatusHandle};

fn main() -> eframe::Result<()> {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();

    // Wire up services
    let counter = Counter::new(0).spawn();
    let status = Status::new("Ready".to_string(), counter.clone()).spawn();

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
