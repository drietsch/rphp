//! `ext/posix` and `ext/pcntl`: the process's identity, users and groups,
//! limits and terminals (`posix_*`), and signals, `fork`, `wait` and `exec`
//! (`pcntl_*`) — the functions and constants php 8.5 declares on the
//! platform it runs on (the tier-A oracle is darwin).
//!
//! Most of it is safe calls through `rustix` and `nix`; what neither wraps
//! is confined to [`sys`], the crate's one `unsafe` module (its doc states
//! why each call is sound). Signals follow php's model: the OS handler only
//! queues the delivery, and the php handlers run later — at
//! `pcntl_signal_dispatch()`, or, under `pcntl_async_signals(true)`, at the
//! interpreter's next safepoint (a loop's back edge or a call), which the
//! handler requests through [`rphp_runtime::Interrupt::poke`] and the
//! interpreter answers by running [`rphp_runtime::Interp::poke_hook`].
//!
//! `pcntl` is registered for the CLI and embed SAPIs only: php builds it
//! for the CLI alone, and `fork()` is only sound in a single-threaded
//! process (see [`sys`]), which a server's worker pool is not.
#![deny(unsafe_code)]
#![deny(unsafe_op_in_unsafe_fn)]
// libc's C types differ by platform (`c_long`, `rlim_t`, the resource
// ids): a cast that is a no-op here keeps the code portable.
#![allow(clippy::unnecessary_cast)]

mod pcntl;
mod posix;
mod sys;

use rphp_runtime::{Interp, Registry, SapiKind};

/// Register the extensions.
pub fn register(r: &mut Registry) {
    r.extension("posix");
    posix::register(r);
    if matches!(r.interp().sapi, SapiKind::Cli | SapiKind::Embed) {
        r.extension("pcntl");
        pcntl::register(r);
    }
}

/// The signal mask a host thread held before [`block_signals`].
#[derive(Clone, Copy)]
pub struct SignalMask(sys::Mask);

/// Block the asynchronous signals on the calling thread, returning its
/// previous mask. The CLI runs a script on a thread of its own while the
/// process's first thread waits for it; that waiting thread blocks signals
/// here, so a process-directed signal — `posix_kill(posix_getpid(), …)`,
/// a terminal's Ctrl-C — is delivered to the script's thread, as it is to
/// php's only thread, before `kill()` returns. The script's thread
/// inherits the blocked mask and puts the saved one back first thing.
pub fn block_signals() -> SignalMask {
    SignalMask(sys::block_async_signals())
}

/// Put a mask [`block_signals`] saved back on the calling thread.
pub fn restore_signals(m: &SignalMask) {
    sys::restore_mask(&m.0);
}

/// Whether this process is a child `pcntl_fork()` made. Such a child runs
/// on the script's thread alone — the host thread that would have turned
/// the script's exit code into the process's is the parent's — so the host
/// ends the child itself, with that code, once the request has finished.
pub fn is_forked_child() -> bool {
    sys::forked()
}

/// php's `RSHUTDOWN` for the process-wide signal state: dispositions the
/// request changed go back to the default and async delivery is switched
/// off, so a long-lived host's next request starts clean. The per-request
/// state (handlers, last errors) lives in the interpreter's slots and goes
/// with it.
pub fn request_shutdown(it: &mut Interp) {
    pcntl::request_shutdown(it);
}
