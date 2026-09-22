//! Native replacement for `tauri-plugin-single-instance` (SOU-191).
//!
//! An advisory `flock` on a file under the app data directory decides who is
//! primary; a Unix domain socket next to it lets a second launch wake the
//! first instance's window instead of just exiting silently. No Objective-C
//! needed: `NSDistributedNotificationCenter` would also work but pulls in
//! more uncertain FFI surface for no real benefit over a local socket, since
//! both processes are always the same user on the same machine.

use std::io::Write;
use std::os::fd::IntoRawFd;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;

use tracing::warn;

use crate::constants::app_data_dir;
use crate::native::bridge::{self, NativeAction};

fn lock_path() -> PathBuf {
    app_data_dir().join("souffle.lock")
}

fn socket_path() -> PathBuf {
    app_data_dir().join("souffle.sock")
}

/// Try to become the primary instance. Returns `true` if this process should
/// continue starting up normally, `false` if another instance is already
/// running (in which case it has already been asked to show its window, and
/// the caller should exit immediately without doing any further startup
/// work — no audio thread, no engine actor, no database open).
pub fn acquire_or_notify_existing() -> bool {
    let lock_file = match std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(lock_path())
    {
        Ok(f) => f,
        Err(e) => {
            warn!("Single-instance lock file could not be opened: {e}; continuing anyway");
            return true;
        }
    };

    let fd = lock_file.into_raw_fd();
    let locked = unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) } == 0;

    if locked {
        // Leak the fd deliberately: the lock must live for the process
        // lifetime, and there is no natural owner to hold it otherwise.
        start_socket_listener();
        true
    } else {
        notify_existing_instance();
        false
    }
}

fn start_socket_listener() {
    let path = socket_path();
    let _ = std::fs::remove_file(&path);
    let listener = match UnixListener::bind(&path) {
        Ok(l) => l,
        Err(e) => {
            warn!(
                "Single-instance socket bind failed: {e}; second launches will not raise this window"
            );
            return;
        }
    };
    std::thread::Builder::new()
        .name("single-instance-listener".into())
        .spawn(move || {
            for stream in listener.incoming().flatten() {
                drop(stream);
                bridge::dispatch(NativeAction::ShowMainWindow);
            }
        })
        .expect("failed to spawn single-instance listener thread");
}

fn notify_existing_instance() {
    match UnixStream::connect(socket_path()) {
        Ok(mut stream) => {
            let _ = stream.write_all(b"show");
        }
        Err(e) => warn!("Could not reach the running Soufflé instance: {e}"),
    }
}
