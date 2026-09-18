use std::{collections::HashMap, path::PathBuf};

fn main() {
    // souffle_lib's linker arguments do not propagate to this binary crate.
    // The Swift bridge weak-links @rpath/libswift_Concurrency.dylib; without
    // this runtime search path, TaskPriority metadata is null and its first
    // use aborts even on a supported macOS release. Keep the path on the final
    // executable (also used by the isolated FoundationModels helper).
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!("cargo:rustc-link-arg=-Wl,-rpath,/usr/lib/swift");
    }

    let library = HashMap::from([("lucide".to_string(), PathBuf::from(lucide_slint::lib()))]);
    let config = slint_build::CompilerConfiguration::new().with_library_paths(library);
    slint_build::compile_with_config("ui/main_window.slint", config).expect("Slint build failed");
}
