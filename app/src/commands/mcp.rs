//! Support for the "MCP server" card on the System settings tab: locates the bundled
//! `souffle-mcp` sidecar binary and helps the user wire it into an MCP
//! client (Claude Desktop, Claude Code).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Sidecar binary name once bundled: `scripts/bundle-macos.sh` drops the
/// target-triple suffix when it copies the sidecar next to the app executable.
const MCP_BINARY_NAME: &str = "souffle-mcp";

/// Shell-escape a path for pasting into zsh/bash (e.g. `claude mcp add souffle …`).
/// Bundled installs live under `Soufflé.app` — unquoted paths break copy-paste.
fn shell_escape_path(path: &str) -> String {
    if path.is_empty() {
        return "''".to_string();
    }
    format!("'{}'", path.replace('\'', "'\\''"))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpSetupInfo {
    pub exists: bool,
    pub claude_desktop_snippet: String,
    pub claude_code_command: String,
}

/// Resolve the `souffle-mcp` sidecar path.
///
/// - Release bundle: sits next to the app executable (`Contents/MacOS/` on
///   macOS), where `scripts/bundle-macos.sh` copies it.
/// - Dev: the app binary runs from `target/{debug,release}/souffle`, but the
///   sidecar is built separately (`scripts/build-mcp-sidecar.sh` or
///   `cargo build -p souffle-mcp`) into the sibling `target/{debug,release}/`.
pub fn resolve_mcp_binary_path() -> PathBuf {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from("."));

    let bundled = exe_dir.join(MCP_BINARY_NAME);
    if bundled.is_file() {
        return bundled;
    }

    if let Some(target_dir) = exe_dir.parent() {
        for profile in ["release", "debug"] {
            let candidate = target_dir.join(profile).join(MCP_BINARY_NAME);
            if candidate.is_file() {
                return candidate;
            }
        }
    }

    bundled
}

/// Resolve the sidecar path and build the copy/paste snippets for the settings
/// card. Never fails: an absent binary is a valid (if inactionable) state
/// the UI shows, not an error.
pub fn get_mcp_setup_info() -> Result<McpSetupInfo, String> {
    let path = resolve_mcp_binary_path();
    let exists = path.is_file();
    let path_str = path.to_string_lossy().to_string();

    let claude_desktop_snippet = serde_json::to_string_pretty(&serde_json::json!({
        "mcpServers": {
            "souffle": { "command": path_str }
        }
    }))
    .map_err(|e| format!("Build Claude Desktop snippet: {e}"))?;

    let escaped_path = shell_escape_path(&path_str);

    Ok(McpSetupInfo {
        exists,
        claude_code_command: format!("claude mcp add souffle {escaped_path}"),
        claude_desktop_snippet,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn setup_info_snippet_embeds_resolved_path() {
        let info = get_mcp_setup_info().unwrap();
        let path = resolve_mcp_binary_path().to_string_lossy().to_string();
        let snippet: serde_json::Value =
            serde_json::from_str(&info.claude_desktop_snippet).unwrap();
        assert_eq!(snippet["mcpServers"]["souffle"]["command"], path.as_str());
        assert!(info.claude_desktop_snippet.contains("mcpServers"));
        assert!(info.claude_desktop_snippet.contains("souffle"));
        assert_eq!(
            info.claude_code_command,
            format!("claude mcp add souffle {}", shell_escape_path(&path))
        );
    }

    #[test]
    fn shell_escape_path_quotes_accented_app_bundle_paths() {
        let path = "/Applications/Soufflé.app/Contents/MacOS/souffle-mcp";
        assert_eq!(
            shell_escape_path(path),
            "'/Applications/Soufflé.app/Contents/MacOS/souffle-mcp'"
        );
    }

    #[test]
    fn shell_escape_path_escapes_embedded_single_quotes() {
        assert_eq!(shell_escape_path("it's"), "'it'\\''s'");
    }

    #[test]
    fn resolve_mcp_binary_path_falls_back_to_bundled_guess_when_nothing_found() {
        // No sidecar binary exists in this test environment (or does, if the
        // developer already built it) — either way the resolver must not
        // panic and must return a path ending in the expected binary name.
        let path = resolve_mcp_binary_path();
        assert_eq!(
            path.file_name().and_then(|n| n.to_str()),
            Some(MCP_BINARY_NAME)
        );
    }
}
