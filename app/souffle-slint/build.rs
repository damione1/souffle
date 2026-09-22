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

    let library = HashMap::from([("lucide".to_string(), PathBuf::from(lucide_slint::lib()))]);
    let config = slint_build::CompilerConfiguration::new()
        .with_bundled_translations("lang")
        .with_library_paths(library);
    slint_build::compile_with_config("ui/main_window.slint", config).expect("Slint build failed");
}
