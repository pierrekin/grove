use eframe::egui;
use grove::Event;

// =============================================================================
// Events
// =============================================================================

#[derive(Clone, Debug, Event)]
pub struct CounterIncremented {
    pub new_value: usize,
}

// =============================================================================
// Service
// =============================================================================

#[grove::service]
#[grove(emits = [CounterIncremented])]
pub struct Counter {
    #[grove(get, default)]
    value: usize,
}

#[grove::handlers]
impl Counter {
    #[grove(command)]
    fn increment(&mut self) {
        self.value += 1;
        self.emit_counter_incremented(CounterIncremented {
            new_value: self.value,
        });
    }

    #[grove(command)]
    fn decrement(&mut self) {
        self.value = self.value.saturating_sub(1);
        self.emit_counter_incremented(CounterIncremented {
            new_value: self.value,
        });
    }
}

// =============================================================================
// UI
// =============================================================================

pub fn show(ctx: &egui::Context, handle: &CounterHandle) {
    egui::CentralPanel::default().show(ctx, |ui| {
        ui.heading("Counter");
        ui.add_space(20.0);

        let button_size = egui::vec2(40.0, 40.0);

        ui.horizontal(|ui| {
            if ui.add_sized(button_size, egui::Button::new("-")).clicked() {
                handle.decrement();
            }

            ui.add_sized(
                egui::vec2(80.0, 40.0),
                egui::Label::new(
                    egui::RichText::new(format!("{}", handle.value()))
                        .size(32.0)
                        .monospace(),
                ),
            );

            if ui.add_sized(button_size, egui::Button::new("+")).clicked() {
                handle.increment();
            }
        });
    });
}
