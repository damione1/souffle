use std::ffi::{CStr, CString};
use std::fmt;
use std::io::{Read, Write};
use std::os::raw::{c_char, c_int};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{RecvTimeoutError, sync_channel};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

/// Stable reasons emitted by the FoundationModels bridge when Apple
/// Intelligence cannot run. The Swift bridge is an external boundary: a
/// future OS can add a raw reason, but that value is normalized here instead
/// of leaking strings into the application and Slint contracts.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AppleIntelligenceUnavailableReason {
    DeviceNotEligible,
    AppleIntelligenceNotEnabled,
    ModelNotReady,
    MacosTooOld,
    Stub,
    UnsupportedPlatform,
    Unknown,
}

impl AppleIntelligenceUnavailableReason {
    fn from_external(raw: &str) -> Self {
        match raw {
            "device_not_eligible" => Self::DeviceNotEligible,
            "apple_intelligence_not_enabled" => Self::AppleIntelligenceNotEnabled,
            "model_not_ready" => Self::ModelNotReady,
            "macos_too_old" => Self::MacosTooOld,
            "stub" => Self::Stub,
            "unsupported_platform" => Self::UnsupportedPlatform,
            other => {
                tracing::warn!(
                    reason = other,
                    "Unknown Apple Intelligence availability reason"
                );
                Self::Unknown
            }
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::DeviceNotEligible => "device_not_eligible",
            Self::AppleIntelligenceNotEnabled => "apple_intelligence_not_enabled",
            Self::ModelNotReady => "model_not_ready",
            Self::MacosTooOld => "macos_too_old",
            Self::Stub => "stub",
            Self::UnsupportedPlatform => "unsupported_platform",
            Self::Unknown => "unknown",
        }
    }
}

impl fmt::Display for AppleIntelligenceUnavailableReason {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[repr(C)]
pub struct AppleLLMResponse {
    pub response: *mut c_char,
    pub success: c_int,
    pub error_message: *mut c_char,
}

const HELPER_ARG: &str = "--internal-apple-intelligence-helper";
const MAX_HELPER_FIELD_BYTES: usize = 64 * 1024 * 1024;
const MAX_HELPER_STDERR_BYTES: usize = 1024 * 1024;
const HELPER_DEADLINE: Duration = Duration::from_secs(20);
const HELPER_POLL_INTERVAL: Duration = Duration::from_millis(10);
pub(crate) const HELPER_TIMEOUT_PREFIX: &str = "apple_intelligence_helper_timeout:";
const HELPER_PROCESS_FAILURE_PREFIX: &str = "apple_intelligence_helper_process_failure:";
static APPLE_AI_BRIDGE_HEALTHY: AtomicBool = AtomicBool::new(true);

#[derive(Debug)]
struct HelperOutput {
    status: ExitStatus,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

/// Owns a spawned helper until it has definitely been reaped. Any early
/// return (including BrokenPipe while writing stdin) kills and waits in Drop,
/// so a failed request can never leave a FoundationModels process behind.
struct ChildReaper(Option<Child>);

impl ChildReaper {
    fn child_mut(&mut self) -> &mut Child {
        self.0.as_mut().expect("helper child already reaped")
    }

    fn kill_and_reap(&mut self) {
        if let Some(mut child) = self.0.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    fn finish(&mut self) -> Result<ExitStatus, String> {
        let status = self
            .child_mut()
            .wait()
            .map_err(|error| format!("Wait for Apple Intelligence helper: {error}"))?;
        self.0 = None;
        Ok(status)
    }
}

impl Drop for ChildReaper {
    fn drop(&mut self) {
        self.kill_and_reap();
    }
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
unsafe extern "C" {
    fn is_apple_intelligence_available() -> c_int;
    fn apple_intelligence_unavailable_reason() -> *mut c_char;
    fn process_text_with_system_prompt_apple(
        system_prompt: *const c_char,
        user_content: *const c_char,
        max_tokens: i32,
    ) -> *mut AppleLLMResponse;
    fn free_apple_llm_response(response: *mut AppleLLMResponse);
}

#[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
fn is_apple_intelligence_available() -> c_int {
    0
}

/// Whether this build linked the Apple Intelligence stub instead of FoundationModels.
pub fn is_stub_linked() -> bool {
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        cfg!(apple_intelligence_stub)
    }
    #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
    {
        true
    }
}

/// Whether Apple Intelligence is available on this device at runtime.
pub fn check_apple_intelligence_availability() -> bool {
    if is_stub_linked() {
        return false;
    }
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        unsafe { is_apple_intelligence_available() == 1 }
    }
    #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
    {
        false
    }
}

/// Reason Apple Intelligence is unavailable on this device, or `None` when available.
///
/// On non-macOS/non-Apple-Silicon builds this always reports
/// [`AppleIntelligenceUnavailableReason::UnsupportedPlatform`].
pub fn unavailable_reason() -> Option<AppleIntelligenceUnavailableReason> {
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        let reason_ptr = unsafe { apple_intelligence_unavailable_reason() };
        if reason_ptr.is_null() {
            return None;
        }
        let reason = unsafe { CStr::from_ptr(reason_ptr) }
            .to_string_lossy()
            .into_owned();
        unsafe { libc::free(reason_ptr.cast()) };
        Some(AppleIntelligenceUnavailableReason::from_external(&reason))
    }
    #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
    {
        Some(AppleIntelligenceUnavailableReason::UnsupportedPlatform)
    }
}

/// Run one on-device Foundation Models generation with separate system and user prompts.
pub fn process_text_with_system_prompt(
    system_prompt: &str,
    user_content: &str,
    max_tokens: i32,
) -> Result<String, String> {
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        if !APPLE_AI_BRIDGE_HEALTHY.load(Ordering::Acquire) {
            return Err(
                "Apple Intelligence bridge disabled after a previous process failure".into(),
            );
        }
        let executable = std::env::current_exe()
            .map_err(|e| format!("Resolve Apple Intelligence helper executable: {e}"))?;
        let request = encode_helper_request(system_prompt, user_content, max_tokens)?;
        let mut command = Command::new(executable);
        command.arg(HELPER_ARG);
        let output = match run_helper_process(command, &request, HELPER_DEADLINE) {
            Ok(output) => output,
            Err(error) => {
                APPLE_AI_BRIDGE_HEALTHY.store(false, Ordering::Release);
                return Err(error);
            }
        };
        if !output.status.success() {
            APPLE_AI_BRIDGE_HEALTHY.store(false, Ordering::Release);
            let detail = String::from_utf8_lossy(&output.stderr);
            return Err(format!(
                "{HELPER_PROCESS_FAILURE_PREFIX} helper exited unexpectedly{}",
                if detail.trim().is_empty() {
                    String::new()
                } else {
                    format!(": {}", detail.trim())
                }
            ));
        }
        decode_helper_response(&output.stdout)
    }

    #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
    {
        let _ = (system_prompt, user_content, max_tokens);
        Err("Apple Intelligence is only supported on Apple Silicon macOS".into())
    }
}

/// Whether a bridge error must stop `generate_guarded` without its usual
/// retry. A deadline already killed and reaped the helper; launching the same
/// fragile bridge a second time only repeats the failure and violates the
/// request's wall-clock budget.
pub(crate) fn is_terminal_helper_error(error: &str) -> bool {
    error.starts_with(HELPER_TIMEOUT_PREFIX) || error.starts_with(HELPER_PROCESS_FAILURE_PREFIX)
}

fn encode_helper_request(
    system_prompt: &str,
    user_content: &str,
    max_tokens: i32,
) -> Result<Vec<u8>, String> {
    let mut request = Vec::new();
    write_field(&mut request, system_prompt.as_bytes())?;
    write_field(&mut request, user_content.as_bytes())?;
    request.extend_from_slice(&max_tokens.to_le_bytes());
    Ok(request)
}

fn read_bounded(
    mut reader: impl Read,
    limit: usize,
    stream_name: &'static str,
) -> Result<Vec<u8>, String> {
    let mut kept = Vec::new();
    let mut total = 0usize;
    let mut buffer = [0u8; 8192];
    loop {
        let count = reader
            .read(&mut buffer)
            .map_err(|error| format!("Read Apple Intelligence helper {stream_name}: {error}"))?;
        if count == 0 {
            break;
        }
        total = total.saturating_add(count);
        if kept.len() < limit {
            let remaining = limit - kept.len();
            kept.extend_from_slice(&buffer[..count.min(remaining)]);
        }
    }
    if total > limit {
        return Err(format!(
            "Apple Intelligence helper {stream_name} exceeded {limit} bytes"
        ));
    }
    Ok(kept)
}

fn join_output_reader(
    reader: JoinHandle<Result<Vec<u8>, String>>,
    stream_name: &'static str,
) -> Result<Vec<u8>, String> {
    reader
        .join()
        .map_err(|_| format!("Apple Intelligence helper {stream_name} reader panicked"))?
}

fn join_input_writer(writer: JoinHandle<Result<(), String>>) -> Result<(), String> {
    writer
        .join()
        .map_err(|_| "Apple Intelligence helper stdin writer panicked".to_string())?
}

fn spawn_helper_with_deadline(mut command: Command, deadline_at: Instant) -> Result<Child, String> {
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // A rendezvous channel is intentional: if recv_timeout expires, a late
    // sender cannot deposit an owned Child into an abandoned buffered channel.
    // Its send fails and the spawn thread kills + reaps that late child.
    let (sender, receiver) = sync_channel(0);
    std::thread::Builder::new()
        .name("apple-ai-spawn".into())
        .spawn(move || {
            let result = command
                .spawn()
                .map_err(|error| format!("Start Apple Intelligence helper: {error}"))
                .and_then(|mut child| {
                    if Instant::now() >= deadline_at {
                        let _ = child.kill();
                        let _ = child.wait();
                        Err(format!("{HELPER_TIMEOUT_PREFIX} expired during spawn"))
                    } else {
                        Ok(child)
                    }
                });
            if let Err(send_error) = sender.send(result)
                && let Ok(mut child) = send_error.0
            {
                let _ = child.kill();
                let _ = child.wait();
            }
        })
        .map_err(|error| format!("Start Apple Intelligence spawn supervisor: {error}"))?;

    let remaining = deadline_at.saturating_duration_since(Instant::now());
    match receiver.recv_timeout(remaining) {
        Ok(result) => result,
        Err(RecvTimeoutError::Timeout) => Err(format!(
            "{HELPER_TIMEOUT_PREFIX} expired while spawning helper"
        )),
        Err(RecvTimeoutError::Disconnected) => Err(format!(
            "{HELPER_PROCESS_FAILURE_PREFIX} spawn supervisor disconnected"
        )),
    }
}

fn run_helper_process(
    command: Command,
    request: &[u8],
    deadline: Duration,
) -> Result<HelperOutput, String> {
    // One wall-clock budget covers process creation, request delivery,
    // generation, response draining and process reaping.
    let started = Instant::now();
    let deadline_at = started + deadline;
    let child = spawn_helper_with_deadline(command, deadline_at)?;
    let mut child = ChildReaper(Some(child));
    if started.elapsed() >= deadline {
        return Err(format!(
            "{HELPER_TIMEOUT_PREFIX} exceeded {} ms during spawn",
            deadline.as_millis()
        ));
    }

    let stdin = child
        .child_mut()
        .stdin
        .take()
        .ok_or("Apple Intelligence helper stdin unavailable")?;
    let stdout = child
        .child_mut()
        .stdout
        .take()
        .ok_or("Apple Intelligence helper stdout unavailable")?;
    let stderr = child
        .child_mut()
        .stderr
        .take()
        .ok_or("Apple Intelligence helper stderr unavailable")?;
    let stdout_reader =
        std::thread::spawn(move || read_bounded(stdout, MAX_HELPER_FIELD_BYTES, "stdout"));
    let stderr_reader =
        std::thread::spawn(move || read_bounded(stderr, MAX_HELPER_STDERR_BYTES, "stderr"));
    // The request can exceed the OS pipe capacity. Write it concurrently so
    // a helper that never reads stdin cannot pin the supervising thread past
    // the deadline. The controller always kills and reaps before joining a
    // still-running writer.
    let request = request.to_vec();
    let mut input_writer = Some(std::thread::spawn(move || {
        let mut stdin = stdin;
        stdin
            .write_all(&request)
            .map_err(|error| format!("{HELPER_PROCESS_FAILURE_PREFIX} write request: {error}"))
    }));
    let mut input_written = false;
    let mut status = None;

    loop {
        if !input_written && input_writer.as_ref().is_some_and(JoinHandle::is_finished) {
            let result = join_input_writer(
                input_writer
                    .take()
                    .expect("finished stdin writer disappeared"),
            );
            if let Err(error) = result {
                child.kill_and_reap();
                let _ = join_output_reader(stdout_reader, "stdout");
                let _ = join_output_reader(stderr_reader, "stderr");
                return Err(error);
            }
            input_written = true;
        }

        if status.is_none() {
            match child.child_mut().try_wait() {
                Ok(Some(_)) => status = Some(child.finish()?),
                Ok(None) => {}
                Err(error) => {
                    child.kill_and_reap();
                    if let Some(writer) = input_writer.take() {
                        let _ = join_input_writer(writer);
                    }
                    let _ = join_output_reader(stdout_reader, "stdout");
                    let _ = join_output_reader(stderr_reader, "stderr");
                    return Err(format!(
                        "{HELPER_PROCESS_FAILURE_PREFIX} poll process: {error}"
                    ));
                }
            }
        }

        if status.is_some()
            && input_written
            && stdout_reader.is_finished()
            && stderr_reader.is_finished()
        {
            break;
        }

        if started.elapsed() >= deadline {
            // Kill/reap first: this closes the child's pipe ends and unblocks
            // a writer stuck on a full stdin pipe. Only then is joining safe.
            child.kill_and_reap();
            if let Some(writer) = input_writer.take() {
                let _ = join_input_writer(writer);
            }
            let _ = join_output_reader(stdout_reader, "stdout");
            let _ = join_output_reader(stderr_reader, "stderr");
            return Err(format!(
                "{HELPER_TIMEOUT_PREFIX} exceeded {} ms",
                deadline.as_millis()
            ));
        }

        std::thread::sleep(HELPER_POLL_INTERVAL.min(deadline.saturating_sub(started.elapsed())));
    }

    let status = status.expect("completed helper had no exit status");
    let stdout = join_output_reader(stdout_reader, "stdout")?;
    let stderr = join_output_reader(stderr_reader, "stderr")?;
    Ok(HelperOutput {
        status,
        stdout,
        stderr,
    })
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn process_text_in_helper(
    system_prompt: &str,
    user_content: &str,
    max_tokens: i32,
) -> Result<String, String> {
    let system_cstr = CString::new(system_prompt).map_err(|e| e.to_string())?;
    let user_cstr = CString::new(user_content).map_err(|e| e.to_string())?;

    let response_ptr = unsafe {
        process_text_with_system_prompt_apple(system_cstr.as_ptr(), user_cstr.as_ptr(), max_tokens)
    };

    if response_ptr.is_null() {
        return Err("Null response from Apple Intelligence".into());
    }

    let response = unsafe { &*response_ptr };

    let result = if response.success == 1 {
        if response.response.is_null() {
            Ok(String::new())
        } else {
            let c_str = unsafe { CStr::from_ptr(response.response) };
            Ok(c_str.to_string_lossy().into_owned())
        }
    } else {
        let error_c_str = if !response.error_message.is_null() {
            unsafe { CStr::from_ptr(response.error_message) }
        } else {
            c"Unknown error"
        };
        Err(error_c_str.to_string_lossy().into_owned())
    };

    unsafe { free_apple_llm_response(response_ptr) };

    result
}

fn write_field(writer: &mut impl Write, value: &[u8]) -> Result<(), String> {
    let length = u64::try_from(value.len()).map_err(|_| "Apple Intelligence field too large")?;
    writer
        .write_all(&length.to_le_bytes())
        .and_then(|()| writer.write_all(value))
        .map_err(|e| format!("Write Apple Intelligence helper field: {e}"))
}

fn read_field(reader: &mut impl Read) -> Result<Vec<u8>, String> {
    let mut length = [0u8; 8];
    reader
        .read_exact(&mut length)
        .map_err(|e| format!("Read Apple Intelligence helper field length: {e}"))?;
    let length = usize::try_from(u64::from_le_bytes(length))
        .map_err(|_| "Apple Intelligence helper field length overflow")?;
    if length > MAX_HELPER_FIELD_BYTES {
        return Err("Apple Intelligence helper field exceeds safety limit".into());
    }
    let mut value = vec![0u8; length];
    reader
        .read_exact(&mut value)
        .map_err(|e| format!("Read Apple Intelligence helper field: {e}"))?;
    Ok(value)
}

fn encode_helper_response(result: Result<String, String>) -> Result<Vec<u8>, String> {
    let (status, text) = match result {
        Ok(text) => (1u8, text),
        Err(error) => (0u8, error),
    };
    let mut encoded = vec![status];
    write_field(&mut encoded, text.as_bytes())?;
    Ok(encoded)
}

fn decode_helper_response(encoded: &[u8]) -> Result<String, String> {
    let Some((&status, payload)) = encoded.split_first() else {
        return Err("Apple Intelligence helper returned an empty response".into());
    };
    let text = String::from_utf8(read_field(&mut &*payload)?)
        .map_err(|e| format!("Apple Intelligence helper returned invalid UTF-8: {e}"))?;
    match status {
        1 => Ok(text),
        0 => Err(text),
        value => Err(format!(
            "Apple Intelligence helper returned invalid status {value}"
        )),
    }
}

/// Runs the FoundationModels FFI only in a disposable child process. Swift
/// `fatalError`/ABI aborts cannot unwind through C into Rust; isolating this
/// call turns those process-fatal failures into a normal non-zero exit that
/// the parent converts to a polish fallback.
pub(crate) fn try_run_helper(args: &[String]) -> Option<i32> {
    if args.get(1).map(String::as_str) != Some(HELPER_ARG) {
        return None;
    }
    let result = (|| -> Result<String, String> {
        let mut stdin = std::io::stdin().lock();
        let system = String::from_utf8(read_field(&mut stdin)?)
            .map_err(|e| format!("Invalid helper system prompt: {e}"))?;
        let user = String::from_utf8(read_field(&mut stdin)?)
            .map_err(|e| format!("Invalid helper user content: {e}"))?;
        let mut max_tokens = [0u8; 4];
        stdin
            .read_exact(&mut max_tokens)
            .map_err(|e| format!("Read helper token limit: {e}"))?;
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        {
            process_text_in_helper(&system, &user, i32::from_le_bytes(max_tokens))
        }
        #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
        {
            let _ = (system, user, max_tokens);
            Err("Apple Intelligence helper is unavailable on this platform".into())
        }
    })();
    let encoded = match encode_helper_response(result) {
        Ok(encoded) => encoded,
        Err(error) => {
            eprintln!("Encode Apple Intelligence helper response: {error}");
            return Some(1);
        }
    };
    match std::io::stdout().write_all(&encoded) {
        Ok(()) => Some(0),
        Err(error) => {
            eprintln!("Write Apple Intelligence helper response: {error}");
            Some(1)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AppleIntelligenceUnavailableReason, check_apple_intelligence_availability,
        decode_helper_response, encode_helper_response, is_stub_linked, read_bounded,
        run_helper_process, unavailable_reason,
    };
    use std::io::Cursor;
    use std::process::Command;
    use std::time::{Duration, Instant};

    #[test]
    fn availability_check_does_not_panic() {
        let _available = check_apple_intelligence_availability();
    }

    #[test]
    fn stub_build_never_reports_available() {
        if is_stub_linked() {
            assert!(!check_apple_intelligence_availability());
        }
    }

    #[test]
    fn unavailable_reason_is_coherent_with_availability() {
        let available = check_apple_intelligence_availability();
        let reason = unavailable_reason();
        assert_eq!(available, reason.is_none());
        if is_stub_linked() {
            assert_eq!(reason, Some(AppleIntelligenceUnavailableReason::Stub));
        }
    }

    #[test]
    fn external_unavailable_reasons_are_normalized_at_the_ffi_boundary() {
        use AppleIntelligenceUnavailableReason as Reason;

        let cases = [
            ("device_not_eligible", Reason::DeviceNotEligible),
            (
                "apple_intelligence_not_enabled",
                Reason::AppleIntelligenceNotEnabled,
            ),
            ("model_not_ready", Reason::ModelNotReady),
            ("macos_too_old", Reason::MacosTooOld),
            ("stub", Reason::Stub),
            ("unsupported_platform", Reason::UnsupportedPlatform),
            ("unknown:futureReason", Reason::Unknown),
        ];

        for (raw, expected) in cases {
            assert_eq!(Reason::from_external(raw), expected);
        }
    }

    #[test]
    fn helper_protocol_round_trips_success_and_error() {
        let success = encode_helper_response(Ok("bonjour".into())).unwrap();
        assert_eq!(decode_helper_response(&success).as_deref(), Ok("bonjour"));

        let failure = encode_helper_response(Err("provider failed".into())).unwrap();
        assert_eq!(
            decode_helper_response(&failure).unwrap_err(),
            "provider failed"
        );
    }

    #[test]
    fn bounded_reader_rejects_oversized_output_after_draining_it() {
        let error = read_bounded(Cursor::new(b"123456"), 5, "test").unwrap_err();
        assert!(error.contains("exceeded 5 bytes"), "{error}");
    }

    #[cfg(unix)]
    #[test]
    fn helper_sigabrt_is_reaped_without_aborting_the_parent() {
        use std::os::unix::process::ExitStatusExt;

        let mut command = Command::new("/bin/sh");
        command.args(["-c", "kill -ABRT $$"]);
        let output = run_helper_process(command, &[], Duration::from_secs(2))
            .expect("helper signal is an exit status, not a parent abort");
        assert_eq!(output.status.signal(), Some(libc::SIGABRT));
        assert!(!output.status.success());
        assert!(super::is_terminal_helper_error(
            super::HELPER_PROCESS_FAILURE_PREFIX
        ));
    }

    #[cfg(unix)]
    #[test]
    fn helper_deadline_kills_and_reaps_the_process() {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "sleep 5"]);
        let started = Instant::now();
        let error = run_helper_process(command, &[], Duration::from_millis(40)).unwrap_err();
        assert!(
            error.starts_with(super::HELPER_TIMEOUT_PREFIX),
            "unexpected error: {error}"
        );
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "deadline did not terminate the helper promptly"
        );
    }

    #[cfg(unix)]
    #[test]
    fn helper_deadline_interrupts_a_writer_blocked_on_full_stdin_pipe() {
        let mut command = Command::new("/bin/sh");
        // `sleep` inherits stdin but never reads it. The payload is much
        // larger than an OS pipe, so the writer remains blocked until the
        // supervisor reaches its deadline, kills and reaps the child.
        command.args(["-c", "exec sleep 5"]);
        let request = vec![0u8; 8 * 1024 * 1024];
        let started = Instant::now();
        let error = run_helper_process(command, &request, Duration::from_millis(40)).unwrap_err();
        assert!(
            error.starts_with(super::HELPER_TIMEOUT_PREFIX),
            "unexpected error: {error}"
        );
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "blocked stdin writer escaped the helper deadline"
        );
    }

    #[cfg(unix)]
    #[test]
    fn broken_pipe_terminates_and_reaps_the_process() {
        let mut command = Command::new("/bin/sh");
        // The large request fills the pipe until the child has executed the
        // explicit stdin close, making EPIPE deterministic while it remains
        // alive long enough that ChildReaper must terminate it.
        command.args(["-c", "exec 0<&-; exec sleep 5"]);
        let request = vec![0u8; 8 * 1024 * 1024];
        let started = Instant::now();
        let error = run_helper_process(command, &request, Duration::from_secs(1)).unwrap_err();
        assert!(
            error.starts_with(super::HELPER_PROCESS_FAILURE_PREFIX),
            "unexpected error: {error}"
        );
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "BrokenPipe did not terminate the helper promptly"
        );
    }
}
