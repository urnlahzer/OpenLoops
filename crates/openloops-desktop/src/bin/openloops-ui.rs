#![forbid(unsafe_code)]
#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|arg| arg == "--probe-saved-model") {
        if args.iter().any(|arg| arg == "--compare-decisions") {
            let data = args
                .iter()
                .position(|arg| arg == "--data")
                .and_then(|index| args.get(index + 1))
                .map(std::path::PathBuf::from);
            match openloops_desktop::slint_ui::probe_compare_decisions(data.as_deref()) {
                Ok(lines) => {
                    for line in lines {
                        println!("{line}");
                    }
                }
                Err(error) => {
                    eprintln!("{error}");
                    std::process::exit(1);
                }
            }
            return;
        }
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
