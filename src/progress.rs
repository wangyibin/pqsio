//! Optional thread-local, advisory progress. Never called from worker threads.
use std::{cell::Cell, ffi::c_void};

pub type Callback = unsafe extern "C" fn(*const u8, usize, u64, u64, *mut c_void);
type Observer = Option<(Callback, *mut c_void)>;
thread_local! {
    static OBSERVER: Cell<Observer> = const { Cell::new(None) };
    static ACTIVE: Cell<bool> = const { Cell::new(false) };
}

/// # Safety
/// Callback/user must remain valid until cleared on this same thread. The
/// callback must not unwind or reenter pqsio. A null callback clears registration.
pub unsafe fn set(callback: Option<Callback>, user: *mut c_void) {
    OBSERVER.set(callback.map(|f| (f, user)));
}

pub(crate) fn emit(stage: &str, completed: u64, total: u64) {
    if let Some((callback, user)) = OBSERVER.get() {
        if !ACTIVE.replace(true) {
            // SAFETY: registration's caller owns the callback lifetime; stage
            // bytes are borrowed only for the synchronous invocation.
            unsafe { callback(stage.as_ptr(), stage.len(), completed, total, user) };
            ACTIVE.set(false);
        }
    }
}
