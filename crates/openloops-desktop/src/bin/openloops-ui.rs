#![forbid(unsafe_code)]
#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

fn main() {
    if std::env::args().any(|arg| arg == "--probe-saved-model") {
        match openloops_desktop::slint_ui::probe_saved_model() {
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
    if let Err(error) = openloops_desktop::slint_ui::run() {
        eprintln!("OpenLoops UI failed: {error}");
        std::process::exit(1);
    }
}
