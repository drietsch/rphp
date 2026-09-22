//! HTTP's chunked transfer encoding, decoded incrementally (php-src
//! `ext/standard/filters.c`, the `dechunk` filter).
//!
//! A chunked body is a sequence of `<hex size>[;ext]CRLF <size bytes> CRLF`
//! ending with a zero-sized chunk. Nothing may assume a chunk boundary
//! falls where a read does, so this keeps its place across calls: it is
//! what the `http://` wrapper decodes a response with *and* what php's
//! `dechunk` stream filter is.
//!
//! php is lenient here and so is this: a body that is not chunked at all
//! passes through rather than erroring, because php's filter cannot know
//! it was attached in error. Trailing headers after the last chunk are
//! consumed and discarded, as php discards them.

/// Where the decoder is between chunks.
#[derive(Clone, Copy, PartialEq)]
enum At {
    /// Reading the hex size line.
    Size,
    /// Copying a chunk's bytes out.
    Data,
    /// Skipping the CRLF that follows a chunk's bytes.
    AfterData,
    /// Past the zero chunk, skipping trailers to the final blank line.
    Trailers,
    /// Nothing more will come out.
    Done,
}

/// An incremental chunked-body decoder.
pub(crate) struct Dechunk {
    at: At,
    /// Bytes still to copy out of the chunk being read.
    remaining: u64,
    /// The size line, or a trailer line, as it arrives.
    line: Vec<u8>,
}

impl Default for Dechunk {
    fn default() -> Dechunk {
        Dechunk { at: At::Size, remaining: 0, line: Vec::new() }
    }
}

impl Dechunk {
    /// Feed `input`, appending whatever plaintext it yields to `out`.
    pub(crate) fn push(&mut self, input: &[u8], out: &mut Vec<u8>) {
        let mut i = 0;
        while i < input.len() {
            match self.at {
                At::Done => return,
                At::Data => {
                    let take = (input.len() - i).min(self.remaining as usize);
                    out.extend_from_slice(&input[i..i + take]);
                    i += take;
                    self.remaining -= take as u64;
                    if self.remaining == 0 {
                        self.at = At::AfterData;
                        self.line.clear();
                    }
                }
                At::AfterData => {
                    // The CRLF after a chunk's bytes; tolerate a bare LF.
                    if input[i] == b'\n' {
                        self.at = At::Size;
                        self.line.clear();
                    }
                    i += 1;
                }
                At::Size | At::Trailers => {
                    let b = input[i];
                    i += 1;
                    if b != b'\n' {
                        // Guard against a line that never ends.
                        if self.line.len() < 1024 {
                            self.line.push(b);
                        }
                        continue;
                    }
                    let line: Vec<u8> = self.line.drain(..).collect();
                    let line = trim_cr(&line);
                    if self.at == At::Trailers {
                        // A blank line ends the trailers, and the body.
                        if line.is_empty() {
                            self.at = At::Done;
                        }
                        continue;
                    }
                    match chunk_size(line) {
                        Some(0) => self.at = At::Trailers,
                        Some(n) => {
                            self.remaining = n;
                            self.at = At::Data;
                        }
                        // Not a size line at all: php passes a body that
                        // was never chunked through untouched.
                        None => {
                            out.extend_from_slice(line);
                            out.push(b'\n');
                        }
                    }
                }
            }
        }
    }

    /// Whether the terminating chunk has been seen.
    pub(crate) fn is_done(&self) -> bool {
        self.at == At::Done
    }
}

fn trim_cr(line: &[u8]) -> &[u8] {
    match line.split_last() {
        Some((b'\r', rest)) => rest,
        _ => line,
    }
}

/// The hex size at the head of a chunk line, ignoring any `;extension`.
fn chunk_size(line: &[u8]) -> Option<u64> {
    let digits: &[u8] = match line.iter().position(|&b| b == b';') {
        Some(i) => &line[..i],
        None => line,
    };
    let digits = digits.strip_prefix(b" ").unwrap_or(digits);
    if digits.is_empty() || !digits.iter().all(u8::is_ascii_hexdigit) {
        return None;
    }
    let text = std::str::from_utf8(digits).ok()?;
    u64::from_str_radix(text, 16).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all(input: &[&[u8]]) -> Vec<u8> {
        let mut d = Dechunk::default();
        let mut out = Vec::new();
        for part in input {
            d.push(part, &mut out);
        }
        out
    }

    #[test]
    fn one_piece() {
        assert_eq!(all(&[b"5\r\nHello\r\n6\r\n world\r\n0\r\n\r\n"]), b"Hello world");
    }

    #[test]
    fn split_anywhere() {
        // The same body, delivered one byte at a time, decodes the same.
        let body = b"5\r\nHello\r\n6\r\n world\r\n0\r\n\r\n";
        let parts: Vec<&[u8]> = body.chunks(1).collect();
        assert_eq!(all(&parts), b"Hello world");
        // …and split at every other boundary too.
        for n in 2..body.len() {
            let parts: Vec<&[u8]> = body.chunks(n).collect();
            assert_eq!(all(&parts), b"Hello world", "split into {n}-byte pieces");
        }
    }

    #[test]
    fn chunk_extensions_are_ignored() {
        assert_eq!(all(&[b"5;name=value\r\nHello\r\n0\r\n\r\n"]), b"Hello");
    }

    #[test]
    fn trailers_are_dropped() {
        assert_eq!(all(&[b"2\r\nhi\r\n0\r\nX-Checksum: abc\r\n\r\n"]), b"hi");
    }

    #[test]
    fn a_body_that_was_never_chunked_passes_through() {
        assert_eq!(all(&[b"not chunked at all\n"]), b"not chunked at all\n");
    }

    #[test]
    fn done_is_reported() {
        let mut d = Dechunk::default();
        let mut out = Vec::new();
        d.push(b"1\r\na\r\n", &mut out);
        assert!(!d.is_done());
        d.push(b"0\r\n\r\n", &mut out);
        assert!(d.is_done());
        assert_eq!(out, b"a");
    }
}
