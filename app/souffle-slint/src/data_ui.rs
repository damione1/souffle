//! Data + About tabs (SOU-188, remaining "Système" scope). See
//! `data_section.slint`'s doc comment for why "Export archive" uses a
//! native `osascript` folder picker instead of a Tauri dialog plugin.

use crate::MainWindow;
use souffle_lib::archive::DataStats;
use souffle_lib::commands::McpSetupInfo;
use souffle_lib::settings::AppSettings;
use souffle_lib::summary::SummaryProviderChoice;

pub fn populate(window: &MainWindow, settings: &AppSettings) {
    window.set_settings_meeting_audio_retention(crate::settings_ui::audio_retention_to_slint(
        settings.meeting_audio_retention,
    ));
    window.set_settings_auto_update_check(settings.auto_update_check_enabled);
    window.set_settings_about_transcription_label(settings.transcription_model_id.as_str().into());
    window.set_settings_about_summary_label(summary_label(settings).into());
}

fn summary_label(settings: &AppSettings) -> String {
    match settings.summary_provider {
        SummaryProviderChoice::AppleIntelligence => "Apple Intelligence".to_string(),
        SummaryProviderChoice::Ollama => {
            if settings.ollama_model.is_empty() {
                "Ollama".to_string()
            } else {
                settings.ollama_model.clone()
            }
        }
        SummaryProviderChoice::Auto => "Automatique".to_string(),
    }
}

fn format_bytes(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;
    let bytes = bytes as f64;
    if bytes >= GB {
        format!("{:.1} Go", bytes / GB)
    } else if bytes >= MB {
        format!("{:.1} Mo", bytes / MB)
    } else if bytes >= KB {
        format!("{:.0} Ko", bytes / KB)
    } else {
        format!("{bytes:.0} o")
    }
}

pub fn populate_stats(window: &MainWindow, stats: &DataStats) {
    window.set_settings_data_stats_line(
        format!(
            "{} \u{b7} {} réunions \u{b7} {} dictées",
            format_bytes(stats.db_size_bytes),
            stats.meeting_count,
            stats.dictation_count
        )
        .into(),
    );
    window.set_settings_data_recordings_line(if stats.recordings_size_bytes > 0 {
        format!(
            "Enregistrements audio : {}",
            format_bytes(stats.recordings_size_bytes)
        )
        .into()
    } else {
        "".into()
    });
}

pub fn populate_mcp(window: &MainWindow, info: &McpSetupInfo) {
    window.set_settings_mcp_binary_path(info.binary_path.as_str().into());
    window.set_settings_mcp_exists(info.exists);
    window.set_settings_mcp_claude_desktop_snippet(info.claude_desktop_snippet.as_str().into());
    window.set_settings_mcp_claude_code_command(info.claude_code_command.as_str().into());
}
