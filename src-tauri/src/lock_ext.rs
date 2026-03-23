use std::sync::{Mutex, MutexGuard};

/// Extension trait for cleaner mutex locking with consistent error messages.
pub trait MutexExt<T> {
    fn acquire(&self) -> Result<MutexGuard<'_, T>, String>;
}

impl<T> MutexExt<T> for Mutex<T> {
    fn acquire(&self) -> Result<MutexGuard<'_, T>, String> {
        self.lock().map_err(|e| format!("Lock poisoned: {e}"))
    }
}
