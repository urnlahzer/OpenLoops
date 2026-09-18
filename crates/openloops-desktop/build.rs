#![forbid(unsafe_code)]

#[cfg(feature = "native-ui")]
#[path = "src/winres.rs"]
mod winres;

fn main() {
    #[cfg(feature = "native-ui")]
    {
        compile_ui();
        embed_windows_icon();
    }
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

#[cfg(feature = "native-ui")]
fn embed_windows_icon() {
    const ICON_PATH: &str = "ui/assets/openloops.ico";

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows")
        || std::env::var("CARGO_CFG_TARGET_ENV").as_deref() != Ok("msvc")
    {
        return;
    }

    println!("cargo:rerun-if-changed={ICON_PATH}");
    let icon = std::fs::read(ICON_PATH)
        .unwrap_or_else(|error| panic!("failed to read Windows icon {ICON_PATH}: {error}"));
    let resource = winres::icon_to_res(&icon)
        .unwrap_or_else(|error| panic!("failed to embed Windows icon {ICON_PATH}: {error}"));
    let output_path = std::path::PathBuf::from(
        std::env::var_os("OUT_DIR").expect("Cargo did not set OUT_DIR for build script"),
    )
    .join("openloops-icon.res");
    std::fs::write(&output_path, resource).unwrap_or_else(|error| {
        panic!(
            "failed to write Windows icon resource {}: {error}",
            output_path.display()
        )
    });
    println!("cargo:rustc-link-arg-bins={}", output_path.display());
}
