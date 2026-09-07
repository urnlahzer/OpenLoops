#![forbid(unsafe_code)]
#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

#[path = "../deadline_view.rs"]
mod deadline_view;
#[path = "../loop_state.rs"]
mod loop_state;
#[path = "../review_ui.rs"]
mod review_ui;
#[path = "../settings.rs"]
mod settings;
#[path = "../setup_ui.rs"]
mod setup_ui;

fn main() {
    if std::env::args().any(|arg| arg == "--probe-saved-model") {
        let result = (|| {
            let store = settings::production_store()
                .map_err(|_| "Saved settings unavailable".to_string())?
                .ok_or("Saved settings disabled".to_string())?;
            let settings = store
                .load()
                .map_err(|_| "Saved settings could not be loaded".to_string())?
                .ok_or("No saved settings".to_string())?;
            review_ui::probe(
                settings.provider,
                settings.active_key().to_string(),
                settings.active_model(),
            )
            .map_err(|error| error.to_string())
        })();
        match result {
            Ok(count) => {
                println!(
                    "Live selected-model semantic smoke check passed: {count} conversation cases."
                );
            }
            Err(error) => {
                eprintln!("{error}");
                std::process::exit(1);
            }
        }
        return;
    }
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([1100.0, 850.0])
            .with_min_inner_size([820.0, 680.0]),
        ..Default::default()
    };
    let _ = eframe::run_native(
        "OpenLoops",
        options,
        Box::new(|cc| Ok(Box::new(setup_ui::SetupApp::new(&cc.egui_ctx)))),
    );
}
