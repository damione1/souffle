//! Settings > Data "Connect to AI assistants" support: locates the bundled
//! `souffle-mcp` sidecar binary and helps the user wire it into an MCP
//! client (Claude Desktop, Claude Code).

use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Sidecar binary name once bundled — Tauri strips the target-triple suffix
/// (`souffle-mcp-aarch64-apple-darwin`) when it copies `externalBin` entries
/// into the app bundle, leaving just this name next to the app executable.
const MCP_BINARY_NAME: &str = "souffle-mcp";

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
pub struct McpSetupInfo {
    pub binary_path: String,
    pub exists: bool,
    pub claude_desktop_snippet: String,
    pub claude_code_command: String,
}

/// Resolve the `souffle-mcp` sidecar path.
///
/// - Release bundle: sits next to the app executable (`Contents/MacOS/` on
///   macOS), because Tauri copies `externalBin` entries there at build time.
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

/// Resolve the sidecar path and build the copy/paste snippets for Settings >
/// Data. Never fails: an absent binary is a valid (if inactionable) state
/// the UI shows, not an error.
#[tauri::command]
#[specta::specta]
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

    Ok(McpSetupInfo {
        exists,
        claude_code_command: format!("claude mcp add souffle {path_str}"),
        binary_path: path_str,
        claude_desktop_snippet,
    })
}

/// Spawn the sidecar, perform an MCP `initialize` handshake and `tools/list`
/// call over stdio, and return the discovered tool names joined by ", ".
/// Used by the Settings UI's "Test connection" button as a quick smoke test
/// that the binary actually speaks MCP.
#[tauri::command]
#[specta::specta]
pub fn test_mcp_connection() -> Result<String, String> {
    let path = resolve_mcp_binary_path();
    if !path.is_file() {
        return Err(format!(
            "Sidecar binary not found at {}. Build it with scripts/build-mcp-sidecar.sh.",
            path.display()
        ));
    }

    let mut child = Command::new(&path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Spawn sidecar: {e}"))?;

    let result = run_handshake(&mut child);
    let _ = child.kill();
    let _ = child.wait();
    result
}

fn run_handshake(child: &mut Child) -> Result<String, String> {
    let mut stdin = child.stdin.take().ok_or("Sidecar has no stdin handle")?;
    let stdout = child.stdout.take().ok_or("Sidecar has no stdout handle")?;
    let mut stderr = child.stderr.take();

    let (tx, rx) = mpsc::channel::<String>();
    std::thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        loop {
            let mut line = String::new();
            match reader.read_line(&mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    if tx.send(line).is_err() {
                        break;
                    }
                }
            }
        }
    });

    send_line(
        &mut stdin,
        &serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": { "name": "souffle-settings", "version": env!("CARGO_PKG_VERSION") },
            },
        }),
    )?;
    recv_response(&rx, &mut stderr)?;

    send_line(
        &mut stdin,
        &serde_json::json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
    )?;

    send_line(
        &mut stdin,
        &serde_json::json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }),
    )?;
    let response = recv_response(&rx, &mut stderr)?;

    let names: Vec<String> = response["result"]["tools"]
        .as_array()
        .ok_or("Malformed tools/list response from sidecar")?
        .iter()
        .filter_map(|tool| tool["name"].as_str().map(str::to_string))
        .collect();

    if names.is_empty() {
        return Err("Sidecar reported no tools".to_string());
    }

    Ok(names.join(", "))
}

fn send_line(stdin: &mut impl Write, value: &serde_json::Value) -> Result<(), String> {
    writeln!(stdin, "{value}").map_err(|e| format!("Write to sidecar stdin: {e}"))?;
    stdin.flush().map_err(|e| format!("Flush sidecar stdin: {e}"))
}

fn recv_response(
    rx: &mpsc::Receiver<String>,
    stderr: &mut Option<impl Read>,
) -> Result<serde_json::Value, String> {
    let line = rx.recv_timeout(HANDSHAKE_TIMEOUT).map_err(|_| {
        let mut output = String::new();
        if let Some(err) = stderr.as_mut() {
            let _ = err.read_to_string(&mut output);
        }
        if output.trim().is_empty() {
            "Timed out waiting for the sidecar to respond".to_string()
        } else {
            output.trim().to_string()
        }
    })?;

    serde_json::from_str(&line).map_err(|e| format!("Parse sidecar response: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn setup_info_snippet_embeds_resolved_path() {
        let info = get_mcp_setup_info().unwrap();
        assert!(info.claude_desktop_snippet.contains(&info.binary_path));
        assert!(info.claude_desktop_snippet.contains("mcpServers"));
        assert!(info.claude_desktop_snippet.contains("souffle"));
        assert_eq!(
            info.claude_code_command,
            format!("claude mcp add souffle {}", info.binary_path)
        );
    }

    #[test]
    fn resolve_mcp_binary_path_falls_back_to_bundled_guess_when_nothing_found() {
        // No sidecar binary exists in this test environment (or does, if the
        // developer already built it) — either way the resolver must not
        // panic and must return a path ending in the expected binary name.
        let path = resolve_mcp_binary_path();
        assert_eq!(path.file_name().and_then(|n| n.to_str()), Some(MCP_BINARY_NAME));
    }

    #[test]
    fn test_mcp_connection_reports_missing_binary_cleanly() {
        // In CI/test environments the sidecar is very unlikely to be built
        // right next to the test binary; if it happens to exist locally this
        // still exercises the happy path via the handshake instead, which is
        // fine — either way the command must not panic.
        let result = test_mcp_connection();
        if let Err(e) = result {
            assert!(!e.is_empty());
        }
    }
}
