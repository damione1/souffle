//! Relaunch the running app on the user's request (SOU-126).
//!
//! A microphone stuck inside CoreAudio cannot be freed from inside the
//! process: its `mic-open` thread cannot be killed safely, and only a new
//! process gets a clean HAL client. The updater relaunches too, but it opens
//! the replaced bundle right before exiting; a plain restart cannot do that,
//! because the new instance would find this one still holding the
//! single-instance lock, raise its window and exit. So the relaunch waits
//! for this process to be gone first.

use std::process::{Command, Stdio};

/// Arrange for the running `.app` bundle to be opened again once this
/// process has exited. The caller then quits the way it normally does
/// (flushing drafts, finalizing a recording); nothing here exits.
///
/// A detached `/bin/sh` polls this PID and runs `open` on the bundle when
/// it disappears. It is reparented to launchd when this process exits.
pub fn relaunch_after_exit() -> Result<(), String> {
    let bundle_path = super::updater::running_bundle_path()?;
    Command::new("/bin/sh")
        .arg("-c")
        .arg(r#"while /bin/kill -0 "$1" 2>/dev/null; do /bin/sleep 0.2; done; exec /usr/bin/open "$2""#)
        .arg("souffle-relaunch")
        .arg(std::process::id().to_string())
        .arg(&bundle_path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("Schedule relaunch: {e}"))
}
