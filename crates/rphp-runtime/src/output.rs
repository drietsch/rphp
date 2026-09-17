//! The output layer (php-src `main/output.c`): a stack of `ob_start` levels
//! over a streaming sink.
//!
//! `echo` appends to the top level when one is active and otherwise goes
//! **straight to the sink** (`implicit_flush=1` semantics), so a long-running
//! script streams and nothing echoed before a fatal error is lost. Natives that
//! write through [`OutputStack::buf`] land in a small level-0 staging buffer
//! that the interpreter flushes after every native call and before every
//! direct `echo`, which keeps the byte order exact.
//!
//! Handler callbacks (`ob_start($cb)`) need the interpreter to call back into
//! PHP, so the level bookkeeping lives here and the callback protocol lives in
//! [`crate::Interp`] (`ob_start`, `ob_discard_top`, `ob_flush_top`).

use std::cell::RefCell;
use std::rc::Rc;

use rphp_value::Value;

/// `PHP_OUTPUT_HANDLER_START`: first invocation of a handler.
pub const PHP_OUTPUT_HANDLER_START: i64 = 1;
/// `PHP_OUTPUT_HANDLER_CLEAN`: the buffer is being discarded.
pub const PHP_OUTPUT_HANDLER_CLEAN: i64 = 2;
/// `PHP_OUTPUT_HANDLER_FLUSH`: the buffer is being flushed, the level stays.
pub const PHP_OUTPUT_HANDLER_FLUSH: i64 = 4;
/// `PHP_OUTPUT_HANDLER_FINAL`: last invocation, the level is removed.
pub const PHP_OUTPUT_HANDLER_FINAL: i64 = 8;
/// `PHP_OUTPUT_HANDLER_CLEANABLE`.
pub const PHP_OUTPUT_HANDLER_CLEANABLE: i64 = 16;
/// `PHP_OUTPUT_HANDLER_FLUSHABLE`.
pub const PHP_OUTPUT_HANDLER_FLUSHABLE: i64 = 32;
/// `PHP_OUTPUT_HANDLER_REMOVABLE`.
pub const PHP_OUTPUT_HANDLER_REMOVABLE: i64 = 64;
/// `PHP_OUTPUT_HANDLER_STDFLAGS` = cleanable | flushable | removable.
pub const PHP_OUTPUT_HANDLER_STDFLAGS: i64 = 112;
/// `PHP_OUTPUT_HANDLER_STARTED` (status flag).
pub const PHP_OUTPUT_HANDLER_STARTED: i64 = 4096;
/// `PHP_OUTPUT_HANDLER_DISABLED` (status flag).
pub const PHP_OUTPUT_HANDLER_DISABLED: i64 = 8192;
/// `PHP_OUTPUT_HANDLER_PROCESSED` (status flag).
pub const PHP_OUTPUT_HANDLER_PROCESSED: i64 = 16384;
/// Internal handler-type bit php reports for a user callback (`type` = 1).
pub const PHP_OUTPUT_HANDLER_USER: i64 = 1;

/// Where the bytes finally go: the SAPI's stdout, an HTTP response body, a
/// test buffer. `write` must not buffer indefinitely — `flush` is called at
/// exit and after fatal errors, but a streaming sink should write through.
pub trait OutputSink {
    /// Deliver `bytes` (raw; PHP strings are byte strings).
    fn write(&mut self, bytes: &[u8]);
    /// Push everything delivered so far to its destination.
    fn flush(&mut self);
}

/// One `ob_start` level.
pub struct ObLevel {
    /// Buffered bytes not yet passed to the handler / the level below.
    pub buf: Vec<u8>,
    /// The user handler (`ob_start($cb)`), if any.
    pub callback: Option<Value>,
    /// `ob_start(.., $chunk_size)` (recorded; chunked flushing is not
    /// implemented).
    pub chunk_size: usize,
    /// The `PHP_OUTPUT_HANDLER_*` capability flags given to `ob_start`.
    pub flags: i64,
    /// Whether the handler has been invoked at least once (drives the
    /// `PHP_OUTPUT_HANDLER_START` phase bit).
    pub started: bool,
}

impl ObLevel {
    /// The name php reports in `ob_get_status()` / `ob_list_handlers()`.
    pub fn handler_name(&self) -> &'static str {
        if self.callback.is_some() {
            "Closure::__invoke"
        } else {
            "default output handler"
        }
    }
}

/// The output stack: `ob_*` levels over a sink, plus the level-0 staging
/// buffer natives write into.
pub struct OutputStack {
    levels: Vec<ObLevel>,
    pending: Vec<u8>,
    sink: Box<dyn OutputSink>,
}

impl OutputStack {
    /// An empty stack writing through to `sink`.
    pub fn new(sink: Box<dyn OutputSink>) -> OutputStack {
        OutputStack {
            levels: Vec::new(),
            pending: Vec::new(),
            sink,
        }
    }

    /// Replace the sink (the previous one is dropped after a flush).
    pub fn set_sink(&mut self, sink: Box<dyn OutputSink>) {
        self.flush_pending();
        self.sink.flush();
        self.sink = sink;
    }

    /// `echo`: append to the active level, or write straight through.
    pub fn write(&mut self, bytes: &[u8]) {
        if let Some(top) = self.levels.last_mut() {
            top.buf.extend_from_slice(bytes);
        } else {
            self.flush_pending();
            self.sink.write(bytes);
        }
    }

    /// The buffer natives append to: the top level, or the level-0 staging
    /// buffer (flushed by [`OutputStack::flush_pending`]).
    pub fn buf(&mut self) -> &mut Vec<u8> {
        match self.levels.last_mut() {
            Some(top) => &mut top.buf,
            None => &mut self.pending,
        }
    }

    /// Push the level-0 staging buffer to the sink (no-op while a level is
    /// active — the bytes are in that level's buffer).
    pub fn flush_pending(&mut self) {
        if !self.pending.is_empty() {
            let bytes = std::mem::take(&mut self.pending);
            if let Some(top) = self.levels.last_mut() {
                top.buf.extend_from_slice(&bytes);
            } else {
                self.sink.write(&bytes);
            }
        }
    }

    /// Flush staging and the sink itself.
    pub fn flush_sink(&mut self) {
        self.flush_pending();
        self.sink.flush();
    }

    /// Number of active levels (`ob_get_level()`).
    pub fn level(&self) -> usize {
        self.levels.len()
    }

    /// The active levels, bottom first.
    pub fn levels(&self) -> &[ObLevel] {
        &self.levels
    }

    /// Start a level.
    pub fn push(&mut self, callback: Option<Value>, chunk_size: usize, flags: i64) {
        // Anything staged by a native before the level opened belongs below it.
        self.flush_pending();
        self.levels.push(ObLevel {
            buf: Vec::new(),
            callback,
            chunk_size,
            flags,
            started: false,
        });
    }

    /// The top level.
    pub fn top(&self) -> Option<&ObLevel> {
        self.levels.last()
    }

    /// The top level, mutably.
    pub fn top_mut(&mut self) -> Option<&mut ObLevel> {
        self.levels.last_mut()
    }

    /// Remove and return the top level (its buffer untouched).
    pub fn pop(&mut self) -> Option<ObLevel> {
        self.levels.pop()
    }

    /// Length of the top buffer (`ob_get_length()`).
    pub fn top_len(&self) -> Option<usize> {
        self.levels.last().map(|l| l.buf.len())
    }

    /// Copy of the top buffer (`ob_get_contents()`).
    pub fn top_contents(&self) -> Option<Vec<u8>> {
        self.levels.last().map(|l| l.buf.clone())
    }
}

/// An in-memory sink shared with the test that owns it: the interpreter writes
/// through one handle while the test reads the other.
#[derive(Clone, Default)]
pub struct SharedBuffer(Rc<RefCell<Vec<u8>>>);

impl SharedBuffer {
    /// A fresh, empty buffer.
    pub fn new() -> SharedBuffer {
        SharedBuffer::default()
    }

    /// Everything written so far.
    pub fn contents(&self) -> Vec<u8> {
        self.0.borrow().clone()
    }

    /// Everything written so far, leaving the buffer empty.
    pub fn take(&self) -> Vec<u8> {
        std::mem::take(&mut *self.0.borrow_mut())
    }
}

impl OutputSink for SharedBuffer {
    fn write(&mut self, bytes: &[u8]) {
        self.0.borrow_mut().extend_from_slice(bytes);
    }
    fn flush(&mut self) {}
}

/// A sink that discards everything.
pub struct NullSink;

impl OutputSink for NullSink {
    fn write(&mut self, _bytes: &[u8]) {}
    fn flush(&mut self) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn echo_streams_when_no_level_is_active() {
        let sink = SharedBuffer::new();
        let mut out = OutputStack::new(Box::new(sink.clone()));
        out.write(b"a");
        assert_eq!(sink.contents(), b"a");
        out.buf().extend_from_slice(b"b"); // staged by a native
        assert_eq!(sink.contents(), b"a");
        out.write(b"c"); // a direct echo flushes staging first
        assert_eq!(sink.contents(), b"abc");
    }

    #[test]
    fn levels_capture_and_nest() {
        let sink = SharedBuffer::new();
        let mut out = OutputStack::new(Box::new(sink.clone()));
        out.write(b"x");
        out.push(None, 0, PHP_OUTPUT_HANDLER_STDFLAGS);
        out.write(b"in1");
        out.buf().extend_from_slice(b"+n");
        out.push(None, 0, PHP_OUTPUT_HANDLER_STDFLAGS);
        out.write(b"in2");
        assert_eq!(out.level(), 2);
        assert_eq!(out.top_contents().unwrap(), b"in2");
        let inner = out.pop().unwrap();
        out.write(&inner.buf); // ob_end_flush: goes into the level below
        assert_eq!(out.top_contents().unwrap(), b"in1+nin2");
        assert_eq!(out.top_len(), Some(8));
        let outer = out.pop().unwrap();
        assert_eq!(sink.contents(), b"x"); // nothing reached the sink yet
        out.write(&outer.buf);
        out.flush_sink();
        assert_eq!(sink.contents(), b"xin1+nin2");
        assert_eq!(out.level(), 0);
        assert!(out.top_len().is_none());
    }

    #[test]
    fn staged_bytes_join_a_level_opened_later_only_if_still_pending() {
        let sink = SharedBuffer::new();
        let mut out = OutputStack::new(Box::new(sink.clone()));
        out.buf().extend_from_slice(b"pre");
        out.push(None, 0, 0);
        // Opening a level flushes the staging buffer below it first.
        assert_eq!(sink.contents(), b"pre");
        out.buf().extend_from_slice(b"top");
        assert_eq!(out.top_contents().unwrap(), b"top");
    }
}
