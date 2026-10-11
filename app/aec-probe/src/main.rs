//! Development-only SOU-277 probe. Never launched by the shipped app.
mod capture;
mod metrics;
mod native;
mod protocol;

use clap::{Parser, Subcommand};
use std::path::PathBuf;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[derive(Parser)]
#[command(about = "SOU-277 hardware AEC comparison (explicit capture only)")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Create the private protocol template; edit references before recording.
    Init { directory: PathBuf },
    /// Record one trial: baseline exports identical-input raw + FDAF, Apple separate.
    Capture {
        directory: PathBuf,
        #[arg(value_enum)]
        mode: protocol::Mode,
        #[arg(value_enum)]
        scene: protocol::Scene,
        #[arg(value_parser = clap::value_parser!(u8).range(1..=3))]
        repetition: u8,
        /// Record operator reading deviations / route/coexistence details.
        #[arg(long)]
        notes: String,
        /// 60 for coexistence checks; default comparison trials are 30 seconds.
        #[arg(long, default_value_t = 30, value_parser = clap::value_parser!(u32).range(1..=60))]
        seconds: u32,
        /// Play the protocol fixture through native output (positive control only).
        #[arg(long)]
        positive_control: bool,
    },
    /// Run the existing Souffle CLI on one trial with the pinned catalogue profile.
    Transcribe {
        trial: PathBuf,
        #[arg(long)]
        binary: PathBuf,
        #[arg(long)]
        engine: String,
    },
    /// Evaluate successful ASR JSON outputs; incomplete trials remain errors.
    Score {
        trial: PathBuf,
        #[arg(long)]
        engine: String,
    },
    /// Print per-trial metrics and median/range, explicitly listing missing trials.
    Report { directory: PathBuf },
}

fn main() {
    if let Err(e) = run() {
        eprintln!("AEC probe: {e}");
        std::process::exit(1);
    }
}
fn run() -> Result<()> {
    match Cli::parse().command {
        Command::Init { directory } => protocol::init(&directory),
        Command::Capture {
            directory,
            mode,
            scene,
            repetition,
            notes,
            seconds,
            positive_control,
        } => capture::run(
            &directory,
            mode,
            scene,
            repetition,
            &notes,
            seconds,
            positive_control,
        ),
        Command::Transcribe {
            trial,
            binary,
            engine,
        } => metrics::transcribe(&trial, &binary, &engine),
        Command::Score { trial, engine } => metrics::score(&trial, &engine),
        Command::Report { directory } => metrics::report(&directory),
    }
}
