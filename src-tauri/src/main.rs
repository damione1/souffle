// Prevents additional console window on Windows in release
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    if let Some(code) = souffle_lib::cli::try_run_headless() {
        std::process::exit(code);
    }
    souffle_lib::run();
}
