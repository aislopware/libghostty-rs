//! `Sync` stand-ins for [`std::cell::Cell`] and [`std::cell::RefCell`], with
//! the method names the tests use, so that tests can share state with effect
//! callbacks, which must be `Send`, without rewriting every access.
//!
//! Both are a [`Mutex`], which needs only `T: Send` to be `Sync` (a render
//! state is `Send` but not `Sync`). Like `RefCell`, an overlapping borrow
//! panics instead of blocking: the tests use them from one thread, so a held
//! lock can only mean an overlap, which would otherwise deadlock. Inside an
//! effect callback the panic aborts the process, which still fails loudly.

use std::sync::{Mutex, MutexGuard, PoisonError, TryLockError, TryLockResult};

#[track_caller]
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    match mutex.try_lock() {
        Ok(guard) => guard,
        // A test that panicked while holding the lock fails on its own.
        Err(TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
        Err(TryLockError::WouldBlock) => panic!("overlapping borrow of a sync_cell"),
    }
}

fn into_inner<T>(mutex: Mutex<T>) -> T {
    mutex.into_inner().unwrap_or_else(PoisonError::into_inner)
}

/// A `Sync` [`std::cell::Cell`].
#[derive(Debug, Default)]
pub(crate) struct Cell<T>(Mutex<T>);

impl<T> Cell<T> {
    pub(crate) const fn new(value: T) -> Self {
        Self(Mutex::new(value))
    }

    #[track_caller]
    pub(crate) fn set(&self, value: T) {
        *lock(&self.0) = value;
    }
}

impl<T: Copy> Cell<T> {
    #[track_caller]
    pub(crate) fn get(&self) -> T {
        *lock(&self.0)
    }
}

/// A `Sync` [`std::cell::RefCell`].
#[derive(Debug, Default)]
pub(crate) struct RefCell<T>(Mutex<T>);

impl<T> RefCell<T> {
    pub(crate) const fn new(value: T) -> Self {
        Self(Mutex::new(value))
    }

    #[track_caller]
    pub(crate) fn borrow(&self) -> MutexGuard<'_, T> {
        lock(&self.0)
    }

    #[track_caller]
    pub(crate) fn borrow_mut(&self) -> MutexGuard<'_, T> {
        lock(&self.0)
    }

    pub(crate) fn try_borrow_mut(&self) -> TryLockResult<MutexGuard<'_, T>> {
        self.0.try_lock()
    }

    pub(crate) fn into_inner(self) -> T {
        into_inner(self.0)
    }
}

impl<T: Default> RefCell<T> {
    #[track_caller]
    pub(crate) fn take(&self) -> T {
        std::mem::take(&mut *lock(&self.0))
    }
}
