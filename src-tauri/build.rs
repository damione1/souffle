fn main() {
    // ggml-metal (whisper.cpp) uses @available checks; when the deployment
    // target is older than the checked OS version, clang emits calls to
    // __isPlatformVersionAtLeast from compiler-rt, which rustc's link line
    // does not include by default. Link clang's builtins explicitly.
    #[cfg(target_os = "macos")]
    if let Ok(output) = std::process::Command::new("clang")
        .arg("--print-resource-dir")
        .output()
    {
        let resource_dir = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !resource_dir.is_empty() {
            println!("cargo:rustc-link-search=native={resource_dir}/lib/darwin");
            println!("cargo:rustc-link-lib=static=clang_rt.osx");
        }
    }

    tauri_build::build();
}
