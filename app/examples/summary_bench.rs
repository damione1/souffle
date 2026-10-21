//! Opt-in private benchmark; shares the entry point tested by integration tests.
#[path = "../tools/summary_bench.rs"]
mod entry;

fn main() {
    entry::main();
}
