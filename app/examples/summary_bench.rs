//! Opt-in private benchmark executable; never part of the shipped UI.
#[path = "../tests/support/summary_bench.rs"]
mod benchmark;

fn main() {
    // The production Apple bridge re-executes current_exe with its internal
    // helper flag. Dispatch before starting Tokio or reading private inputs.
    if let Some(code) = souffle_lib::cli::try_run_headless() {
        std::process::exit(code);
    }
    tokio::runtime::Runtime::new()
        .expect("benchmark runtime")
        .block_on(benchmark::run_bench());
}
