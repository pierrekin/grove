use crate::counter::{CounterHandle, CounterIncremented};
use eframe::egui;

// =============================================================================
// Service
// =============================================================================

#[grove::service]
pub struct Status {
    #[grove(get)]
    message: String,

    counter: CounterHandle,
}

#[grove::handlers]
impl Status {
    #[grove(from = counter)]
    fn on_counter_incremented(&mut self, event: CounterIncremented) {
        self.message = format!("Counter changed to {}", event.new_value);
    }
}

// =============================================================================
// UI
// =============================================================================

pub fn show(ctx: &egui::Context, handle: &StatusHandle) {
    egui::TopBottomPanel::bottom("status").show(ctx, |ui| {
        ui.label(handle.message());
    });
}
