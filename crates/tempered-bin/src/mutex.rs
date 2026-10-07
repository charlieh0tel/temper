//! Mutex locking where poisoning is not an error.

use std::sync::Mutex;
use std::sync::MutexGuard;
use std::sync::PoisonError;

/// Locks `mutex`, ignoring poisoning: in release builds a panic aborts
/// the process, so no thread can die holding the lock with the data half
/// updated; in tests a panic ends the test anyway.
pub(crate) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}
