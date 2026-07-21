#![forbid(unsafe_code)]

fn main() {
    let status = openloops_application::synthetic_status();
    let all_adapters_disabled = !openloops_graph::is_available()
        && !openloops_inference::is_available()
        && !openloops_persistence::is_available();

    if openloops_contracts::has_no_claims(&status) && all_adapters_disabled {
        println!("OPENLOOPS_SYNTHETIC_SMOKE_OK");
    } else {
        std::process::exit(1);
    }
}
