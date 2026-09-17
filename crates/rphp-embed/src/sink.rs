//! Output sinks for the SAPIs: a streaming stdout sink (`implicit_flush=1`
//! — every write reaches fd 1 immediately, so nothing echoed before a fatal
//! error is lost) and the in-memory buffer tests use.

use std::io::Write;

use rphp_runtime::OutputSink;
pub use rphp_runtime::SharedBuffer as BufferSink;

/// Writes straight through to the process's stdout, flushing after every
/// write. Write errors (a closed pipe) are ignored, as php-cli ignores them.
#[derive(Default)]
pub struct StdoutSink;

impl StdoutSink {
    /// A sink over `std::io::stdout()`.
    pub fn new() -> StdoutSink {
        StdoutSink
    }
}

impl OutputSink for StdoutSink {
    fn write(&mut self, bytes: &[u8]) {
        let mut out = std::io::stdout().lock();
        let _ = out.write_all(bytes);
        let _ = out.flush();
    }

    fn flush(&mut self) {
        let _ = std::io::stdout().lock().flush();
    }
}
