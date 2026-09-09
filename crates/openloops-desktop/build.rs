#![forbid(unsafe_code)]

fn main() {
    #[cfg(feature = "native-ui")]
    compile_ui();
}

// `slint-build` is an optional build-dependency, gated by the `native-ui`
// feature (see Cargo.toml's `[build-dependencies]`/`[features]`). Referencing
// it outside a `#[cfg(feature = "native-ui")]` block fails with E0433 for any
// build without that feature (e.g. `cargo check -p openloops-desktop` with no
// features, or a workspace-wide `--all-targets` build), since the crate
// simply isn't compiled in for those builds.
#[cfg(feature = "native-ui")]
fn compile_ui() {
    slint_build::compile_with_config(
        "ui/app.slint",
        slint_build::CompilerConfiguration::new().with_style("fluent".into()),
    )
    .unwrap_or_else(|error| panic!("failed to compile Slint UI: {error}"));
}
