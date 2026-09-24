//! `openssl_error_string()`: the error queue.
//!
//! OpenSSL keeps a per-thread queue of the errors its routines pushed, and
//! php hands them out one at a time, oldest first, emptying the queue as it
//! goes. The strings are OpenSSL's own — `error:<code>:<library>::<reason>` —
//! so this crate reproduces the codes of the failures it can actually cause
//! (a key that will not decode) rather than inventing a vocabulary. A
//! failure class it does not produce pushes nothing, which is what an
//! OpenSSL build without that code path would also do.

use std::cell::RefCell;

/// OpenSSL 3's DECODER error for input that is not a key it recognises —
/// what php reports after `openssl_pkey_get_public()` refuses a string.
pub const DECODER_UNSUPPORTED: &str = "error:1E08010C:DECODER routines::unsupported";

thread_local! {
    static QUEUE: RefCell<Vec<&'static str>> = const { RefCell::new(Vec::new()) };
}

/// Push an error, as an OpenSSL routine does on its way out.
pub fn push(error: &'static str) {
    QUEUE.with(|q| q.borrow_mut().push(error));
}

/// Take the oldest error, or `None` when the queue is empty.
pub fn take() -> Option<&'static str> {
    QUEUE.with(|q| {
        let mut q = q.borrow_mut();
        if q.is_empty() {
            None
        } else {
            Some(q.remove(0))
        }
    })
}
