//! Settings > Data "Connect to AI assistants" support: locates the bundled
//! `souffle-mcp` sidecar binary and helps the user wire it into an MCP
//! client (Claude Desktop, Claude Code).

use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Sidecar binary name once bundled — Tauri strips the target-triple suffix
/// (`souffle-mcp-aarch64-apple-darwin`) when it copies `externalBin` entries
/// into the app bundle, leaving just this name next to the app executable.
const MCP_BINARY_NAME: &str = "souffle-mcp";

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);
const STDERR_LIMIT: usize = 16 * 1024;

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
        binary_path: path_str,
        claude_desktop_snippet,
    })
}

/// Spawn the sidecar, perform an MCP `initialize` handshake and `tools/list`
/// call over stdio, and return the discovered tool names joined by ", ".
/// Used by the Settings UI's "Test connection" button as a quick smoke test
/// that the binary actually speaks MCP. `tools/list` alone is intentional:
/// it proves stdio JSON-RPC works without needing a populated database.
pub fn test_mcp_connection() -> Result<String, String> {
    let path = resolve_mcp_binary_path();
    if !path.is_file() {
        return Err(format!(
            "Sidecar binary not found at {}. Build it with scripts/build-mcp-sidecar.sh.",
            path.display()
        ));
    }

    let child = Command::new(&path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Spawn sidecar: {e}"))?;

    run_handshake(child, HANDSHAKE_TIMEOUT)
}

fn run_handshake(mut child: Child, timeout: Duration) -> Result<String, String> {
    let mut stdin = child.stdin.take().ok_or("Sidecar has no stdin handle")?;
    let stdout = child.stdout.take().ok_or("Sidecar has no stdout handle")?;
    let stderr = child.stderr.take().ok_or("Sidecar has no stderr handle")?;

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

    // Drain stderr concurrently so a noisy sidecar cannot fill its pipe and
    // deadlock before replying. Keep only a bounded prefix for the UI while
    // continuing to drain the rest.
    let stderr_output = Arc::new(Mutex::new(Vec::new()));
    let stderr_for_reader = Arc::clone(&stderr_output);
    std::thread::spawn(move || drain_stderr(stderr, &stderr_for_reader));

    let result = perform_handshake(&mut stdin, &rx, timeout);
    drop(stdin);

    // `kill` is harmless when the sidecar already exited. Always `wait` so
    // every success, timeout and malformed-response path reaps the child.
    let _ = child.kill();
    let wait_result = child.wait();

    match result {
        Ok(tools) => {
            wait_result.map_err(|e| format!("Reap sidecar: {e}"))?;
            Ok(tools)
        }
        Err(error) => Err(with_stderr(error, &stderr_output)),
    }
}

fn perform_handshake(
    stdin: &mut impl Write,
    rx: &mpsc::Receiver<String>,
    timeout: Duration,
) -> Result<String, String> {
    send_line(
        stdin,
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
    recv_response(rx, timeout)?;

    send_line(
        stdin,
        &serde_json::json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
    )?;

    send_line(
        stdin,
        &serde_json::json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }),
    )?;
    let response = recv_response(rx, timeout)?;

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
    stdin
        .flush()
        .map_err(|e| format!("Flush sidecar stdin: {e}"))
}

fn recv_response(
    rx: &mpsc::Receiver<String>,
    timeout: Duration,
) -> Result<serde_json::Value, String> {
    let line = rx.recv_timeout(timeout).map_err(|error| match error {
        mpsc::RecvTimeoutError::Timeout => {
            "Timed out waiting for the sidecar to respond".to_string()
        }
        mpsc::RecvTimeoutError::Disconnected => {
            "Sidecar closed stdout before responding".to_string()
        }
    })?;

    serde_json::from_str(&line).map_err(|e| format!("Parse sidecar response: {e}"))
}

fn drain_stderr(mut stderr: impl Read, output: &Mutex<Vec<u8>>) {
    let mut chunk = [0_u8; 4096];
    while let Ok(read) = stderr.read(&mut chunk) {
        if read == 0 {
            break;
        }
        if let Ok(mut output) = output.lock() {
            let remaining = STDERR_LIMIT.saturating_sub(output.len());
            output.extend_from_slice(&chunk[..read.min(remaining)]);
        }
    }
}

fn with_stderr(error: String, stderr: &Mutex<Vec<u8>>) -> String {
    let output = stderr
        .lock()
        .ok()
        .map(|output| String::from_utf8_lossy(&output).trim().to_string())
        .unwrap_or_default();
    if output.is_empty() {
        error
    } else {
        format!("{error}: {output}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[test]
    fn setup_info_snippet_embeds_resolved_path() {
        let info = get_mcp_setup_info().unwrap();
        assert!(info.claude_desktop_snippet.contains(&info.binary_path));
        assert!(info.claude_desktop_snippet.contains("mcpServers"));
        assert!(info.claude_desktop_snippet.contains("souffle"));
        assert_eq!(
            info.claude_code_command,
            format!(
                "claude mcp add souffle {}",
                shell_escape_path(&info.binary_path)
            )
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

    #[test]
    fn handshake_timeout_drains_and_bounds_stderr_before_reaping_child() {
        let child = Command::new("sh")
            .arg("-c")
            .arg("yes x | head -c 32768 >&2; printf partial; sleep 30")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let started = Instant::now();

        let error = run_handshake(child, Duration::from_millis(100)).unwrap_err();

        assert!(started.elapsed() < Duration::from_secs(2));
        assert!(error.starts_with("Timed out waiting for the sidecar to respond"));
        assert!(error.contains('x'));
        assert!(error.len() <= STDERR_LIMIT + 128);
    }

    #[test]
    fn handshake_accepts_initialize_and_tools_list_responses() {
        let script = concat!(
            "read _request; ",
            "printf '%s\\n' '{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{}}'; ",
            "read _notification; read _request; ",
            "printf '%s\\n' '{\"jsonrpc\":\"2.0\",\"id\":2,\"result\":{\"tools\":[{\"name\":\"search\"},{\"name\":\"export\"}]}}'"
        );
        let child = Command::new("sh")
            .arg("-c")
            .arg(script)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();

        assert_eq!(
            run_handshake(child, Duration::from_secs(1)).unwrap(),
            "search, export"
        );
    }
}
