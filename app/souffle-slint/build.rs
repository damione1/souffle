use std::{collections::HashMap, path::PathBuf};

fn main() {
    let sha = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|sha| sha.trim().to_owned())
        .unwrap_or_else(|| "unavailable".to_owned());
    println!("cargo:rustc-env=SOUFFLE_BUILD_GIT_SHA={sha}");
    println!(
        "cargo:rustc-env=SOUFFLE_BUILD_PROFILE={}",
        std::env::var("PROFILE").unwrap_or_else(|_| "unavailable".into())
    );
    println!("cargo:rerun-if-env-changed=PROFILE");

    // souffle_lib's linker arguments do not propagate to this binary crate.
    // The Swift bridge weak-links @rpath/libswift_Concurrency.dylib; without
    // this runtime search path, TaskPriority metadata is null and its first
    // use aborts even on a supported macOS release. Keep the path on the final
    // executable (also used by the isolated FoundationModels helper).
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!("cargo:rustc-link-arg=-Wl,-rpath,/usr/lib/swift");
    }

    let ui = PathBuf::from("ui");
    let sources = souffle_typography::validate_ui(&ui).expect("Typography contract failed");
    for source in sources {
        println!("cargo:rerun-if-changed={}", source.display());
    }
    println!("cargo:rerun-if-changed=ui");
    let assets = souffle_typography::asset_dir();
    souffle_typography::validate_assets(&assets).expect("Inter font validation failed");
    println!("cargo:rerun-if-changed={}", assets.display());
    let out_dir = PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR"));
    let typography = out_dir.join("typography.slint");
    std::fs::write(&typography, souffle_typography::slint_projection())
        .expect("Typography projection failed");
    let library = HashMap::from([
        ("lucide".to_string(), PathBuf::from(lucide_slint::lib())),
        ("typography".to_string(), typography.clone()),
    ]);
    // Element ids are only kept with debug info; the layout tests in
    // `live_view.rs` look elements up by id (SOU-256). Debug builds only, so
    // release binaries stay exactly as before.
    let debug_build = std::env::var("PROFILE").as_deref() == Ok("debug");
    let manifest_dir = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("manifest dir"));
    let translations = manifest_dir.join("lang");
    let generated = out_dir.join("main_window.rs");
    let config = slint_build::CompilerConfiguration::new()
        .with_bundled_translations(&translations)
        .with_library_paths(library)
        .with_debug_info(debug_build);
    // compile_with_config would watch typography.slint as an input, even
    // though this script creates it after Cargo's build-start timestamp.
    // That makes the next invocation rebuild the whole UI, including the
    // test run immediately after CI's --no-run step. Track every real Slint
    // dependency, while the projection is already covered by the build
    // dependency on souffle-typography.
    let dependencies = slint_build::compile_with_output_path(
        manifest_dir.join("ui/main_window.slint"),
        &generated,
        config,
    )
    .expect("Slint build failed");
    println!("cargo:rerun-if-changed={}", translations.display());
    for dependency in dependencies {
        if dependency != typography {
            println!("cargo:rerun-if-changed={}", dependency.display());
        }
    }
    for variable in [
        "SLINT_STYLE",
        "SLINT_FONT_SIZES",
        "SLINT_SCALE_FACTOR",
        "SLINT_ASSET_SECTION",
        "SLINT_EMBED_RESOURCES",
        "SLINT_EMIT_DEBUG_INFO",
        "SLINT_LIVE_PREVIEW",
        "SLINT_BUNDLE_TRANSLATIONS",
    ] {
        println!("cargo:rerun-if-env-changed={variable}");
    }
    println!(
        "cargo:rustc-env=SLINT_INCLUDE_GENERATED={}",
        generated.display()
    );
}
