//! The one `unsafe` corner of the extension: SQLite calls a user-defined
//! function, aggregate or collation *synchronously* from inside
//! `sqlite3_step()`, on the thread that called it, and the php callable
//! behind it needs the interpreter — which the native that called `step()`
//! holds as `&mut Interp` and cannot hand into a `'static + Send` closure.
//!
//! So every driver call that may run a callback happens inside an
//! [`InterpScope`]: the interpreter's address is parked in a thread-local
//! for the call's duration and [`with_interp`] lends it to the callback.
//! The invariants that make this sound, stated once:
//!
//! * the scope is entered from a native with `ctx: &mut Interp`, which does
//!   not touch `ctx` again until the scope is dropped — every access in
//!   between goes through the parked pointer, so no two live `&mut`
//!   references exist at once;
//! * SQLite never calls back from another thread (the connection is not
//!   shared), and the interpreter is `!Send` anyway: the [`Sendable`]
//!   wrapper only satisfies rusqlite's closure bounds, the value never
//!   leaves this thread;
//! * a callback that re-enters the same connection (a query inside a
//!   user-defined function) is refused by the payload's `RefCell` rather
//!   than by this module.
//!
//! ADR-032 confined `unsafe` to the PCRE2 binding; this module extends that
//! list by exactly one more confined site (see `COVERAGE.md`, "PDO").
#![allow(unsafe_code)]

use std::cell::Cell;
use std::ptr;

use rphp_runtime::Interp;

thread_local! {
    static CURRENT: Cell<*mut Interp> = const { Cell::new(ptr::null_mut()) };
}

/// The interpreter parked for the duration of a driver call; the previous
/// one (a nested call) is restored on drop.
pub struct InterpScope {
    previous: *mut Interp,
}

impl InterpScope {
    /// Park `ctx`. The caller must not use `ctx` until the scope is dropped.
    pub fn enter(ctx: &mut Interp) -> InterpScope {
        let previous = CURRENT.with(|c| c.replace(ctx as *mut Interp));
        InterpScope { previous }
    }
}

impl Drop for InterpScope {
    fn drop(&mut self) {
        CURRENT.with(|c| c.set(self.previous));
    }
}

/// Lend the parked interpreter to a callback; `None` outside any scope.
pub fn with_interp<R>(f: impl FnOnce(&mut Interp) -> R) -> Option<R> {
    let p = CURRENT.with(|c| c.get());
    if p.is_null() {
        return None;
    }
    // SAFETY: `p` was created from a live `&mut Interp` by `InterpScope::enter`
    // on this very thread, the scope has not been dropped (the pointer would
    // be null or the previous scope's), and the native that entered it is
    // suspended inside the driver call, so this is the only reference in use.
    let interp = unsafe { &mut *p };
    Some(f(interp))
}

/// A value rusqlite's closure bounds want `Send`, which never leaves the
/// interpreter's thread.
pub struct Sendable<T>(pub T);

impl<T> Sendable<T> {
    /// The value (a method, so a closure captures the wrapper whole rather
    /// than the field, which would not be `Send`).
    pub fn get(&self) -> &T {
        &self.0
    }
}

thread_local! {
    static PENDING: std::cell::RefCell<Option<rphp_runtime::Unwind>> = const { std::cell::RefCell::new(None) };
}

/// Park the exception a callback raised: rusqlite's error type must be
/// `Send`, an interpreter unwind is not, so the callback reports a plain
/// failure to SQLite and the native that ran the statement picks the real
/// one up with [`take_unwind`].
pub fn stash_unwind(u: rphp_runtime::Unwind) {
    PENDING.with(|p| *p.borrow_mut() = Some(u));
}

/// The exception a callback raised during the last driver call, if any.
pub fn take_unwind() -> Option<rphp_runtime::Unwind> {
    PENDING.with(|p| p.borrow_mut().take())
}

// SAFETY: see the module documentation — the wrapped callable is only ever
// touched on the thread that registered it, from SQLite's synchronous
// callbacks; the `Send` bound is rusqlite's, not a real thread crossing.
unsafe impl<T> Send for Sendable<T> {}
