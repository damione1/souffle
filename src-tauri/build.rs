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

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    build_apple_intelligence_bridge();

    tauri_build::build();
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn build_apple_intelligence_bridge() {
    use std::env;
    use std::path::{Path, PathBuf};
    use std::process::Command;

    const REAL_SWIFT_FILE: &str = "swift/apple_intelligence.swift";
    const STUB_SWIFT_FILE: &str = "swift/apple_intelligence_stub.swift";
    const BRIDGE_HEADER: &str = "swift/apple_intelligence_bridge.h";

    println!("cargo:rerun-if-changed={REAL_SWIFT_FILE}");
    println!("cargo:rerun-if-changed={STUB_SWIFT_FILE}");
    println!("cargo:rerun-if-changed={BRIDGE_HEADER}");
    println!("cargo::rustc-check-cfg=cfg(apple_intelligence_stub)");
    println!("cargo::rustc-check-cfg=cfg(apple_intelligence_real)");

    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR not set"));
    let object_path = out_dir.join("apple_intelligence.o");
    let static_lib_path = out_dir.join("libapple_intelligence.a");

    let sdk_path = env::var("SDKROOT").unwrap_or_else(|_| {
        String::from_utf8(
            Command::new("xcrun")
                .args(["--sdk", "macosx", "--show-sdk-path"])
                .output()
                .expect("Failed to locate macOS SDK")
                .stdout,
        )
        .expect("SDK path is not valid UTF-8")
        .trim()
        .to_string()
    });

    let framework_path =
        Path::new(&sdk_path).join("System/Library/Frameworks/FoundationModels.framework");
    let force_stub = env::var("SOUFFLE_FORCE_AI_STUB").as_deref() == Ok("1");
    let command_line_tools_only = env::var("SWIFTC").is_err() && is_command_line_tools_only();
    if command_line_tools_only && !force_stub {
        println!(
            "cargo:warning=Command Line Tools-only toolchain detected; Apple Intelligence \
             (FoundationModels) needs full Xcode. Falling back to stubs. Install Xcode and run \
             `sudo xcode-select -s /Applications/Xcode.app`, or set SOUFFLE_FORCE_AI_STUB=1 to \
             silence this message."
        );
    }

    let has_foundation_models = framework_path.exists() && !force_stub && !command_line_tools_only;

    // Notarized releases with real Apple Intelligence require Xcode 26+ (FoundationModels
    // in the macOS SDK). CI runners on macos-15 link the stub until the workflow moves to
    // a runner image with that SDK; stub linkage is surfaced at runtime via is_stub_linked().
    let source_file = if has_foundation_models {
        println!("cargo:rustc-cfg=apple_intelligence_real");
        println!("cargo:warning=Building with Apple Intelligence support.");
        REAL_SWIFT_FILE
    } else if framework_path.exists() {
        println!("cargo:rustc-cfg=apple_intelligence_stub");
        println!("cargo:warning=Building Apple Intelligence with stubs.");
        STUB_SWIFT_FILE
    } else {
        println!("cargo:rustc-cfg=apple_intelligence_stub");
        println!("cargo:warning=Apple Intelligence SDK not found. Building with stubs.");
        STUB_SWIFT_FILE
    };

    if !Path::new(source_file).exists() {
        panic!("Source file {source_file} is missing!");
    }

    let swiftc_path = env::var("SWIFTC").unwrap_or_else(|_| {
        String::from_utf8(
            Command::new("xcrun")
                .args(["--find", "swiftc"])
                .output()
                .expect("Failed to locate swiftc")
                .stdout,
        )
        .expect("swiftc path is not valid UTF-8")
        .trim()
        .to_string()
    });

    let toolchain_swift_lib = Path::new(&swiftc_path)
        .parent()
        .and_then(|p| p.parent())
        .map(|root| root.join("lib/swift/macosx"))
        .expect("Unable to determine Swift toolchain lib directory");
    let sdk_swift_lib = Path::new(&sdk_path).join("usr/lib/swift");

    let status = Command::new(&swiftc_path)
        .args([
            "-parse-as-library",
            "-target",
            "arm64-apple-macosx11.0",
            "-sdk",
            &sdk_path,
            "-O",
            "-import-objc-header",
            BRIDGE_HEADER,
            "-c",
            source_file,
            "-o",
            object_path.to_str().expect("object path"),
        ])
        .status()
        .expect("Failed to invoke swiftc for Apple Intelligence bridge");

    if !status.success() {
        panic!("swiftc failed to compile {source_file}");
    }

    let status = Command::new("libtool")
        .args([
            "-static",
            "-o",
            static_lib_path.to_str().expect("static lib path"),
            object_path.to_str().expect("object path"),
        ])
        .status()
        .expect("Failed to create static library for Apple Intelligence bridge");

    if !status.success() {
        panic!("libtool failed for Apple Intelligence bridge");
    }

    println!("cargo:rustc-link-search=native={}", out_dir.display());
    println!("cargo:rustc-link-lib=static=apple_intelligence");
    println!(
        "cargo:rustc-link-search=native={}",
        toolchain_swift_lib.display()
    );
    println!("cargo:rustc-link-search=native={}", sdk_swift_lib.display());
    println!("cargo:rustc-link-lib=framework=Foundation");

    if has_foundation_models {
        println!("cargo:rustc-link-arg=-weak_framework");
        println!("cargo:rustc-link-arg=FoundationModels");
    }

    println!("cargo:rustc-link-arg=-Wl,-rpath,/usr/lib/swift");
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn is_command_line_tools_only() -> bool {
    use std::process::Command;

    Command::new("xcode-select")
        .arg("-p")
        .output()
        .ok()
        .filter(|out| out.status.success())
        .and_then(|out| String::from_utf8(out.stdout).ok())
        .map(|path| path.trim().ends_with("CommandLineTools"))
        .unwrap_or(false)
}
