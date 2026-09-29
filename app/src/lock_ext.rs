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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[test]
    fn acquire_returns_guard() {
        let m = Mutex::new(42);
        let guard = m.acquire().unwrap();
        assert_eq!(*guard, 42);
    }

    #[test]
    fn poisoned_mutex_returns_error() {
        let m = std::sync::Arc::new(Mutex::new(42));
        let m2 = m.clone();
        let _ = std::thread::spawn(move || {
            let _guard = m2.lock().unwrap();
            panic!("intentional panic to poison mutex");
        })
        .join();
        assert!(m.acquire().is_err());
    }
}
