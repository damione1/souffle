//! Native single instance lock replacement for Tauri single-instance plugin.

use std::fs::File;

pub struct SingleInstanceGuard {
    _file: File,
}

pub fn acquire_single_instance_lock() -> Result<SingleInstanceGuard, String> {
    let lock_dir = std::env::temp_dir();
    let lock_path = lock_dir.join("souffle.lock");

    let file = File::create(&lock_path)
        .map_err(|e| format!("Failed to create lockfile {}: {}", lock_path.display(), e))?;

    #[cfg(unix)]
    {
        use std::os::unix::io::AsRawFd;
        let res = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if res != 0 {
            return Err("Another instance of Soufflé is already running".into());
        }
    }

    Ok(SingleInstanceGuard { _file: file })
}
