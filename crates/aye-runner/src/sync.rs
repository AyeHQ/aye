//! What the runner's threads share.

use std::sync::{Mutex, MutexGuard, PoisonError};

/// Lock, ignoring poisoning: a panicked reader thread leaves valid data.
pub(crate) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}
