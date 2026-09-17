//! A minimal, documented FFI layer over PCRE2 (ADR-032: the `pcre` extension
//! stays on the engine php-src links, so pattern semantics match byte for
//! byte). The `pcre2` crate's builder cannot express PHP's modifier set, the
//! match-limit ini directives, start offsets with `NOTEMPTY_ATSTART|ANCHORED`
//! retries, `(*MARK)` or unmatched-group offsets, so the stdlib talks to
//! `pcre2-sys` through this crate instead.
//!
//! This is the **only** crate in the workspace that may use `unsafe`; every
//! `unsafe` block is confined to a resource wrapper ([`Code`],
//! [`MatchContext`], [`MatchData`]) whose invariants are stated on the type.
//! Nothing here is `Send`/`Sync`: a compiled pattern belongs to the request
//! (interpreter) that compiled it.
#![deny(unsafe_op_in_unsafe_fn)]
#![deny(missing_docs)]

use std::ffi::c_void;
use std::ptr;

use pcre2_sys::*;

/// Compile-time option bits accepted by [`Code::compile`] (`pcre2_compile`).
pub mod opt {
    pub use pcre2_sys::{
        PCRE2_ANCHORED as ANCHORED, PCRE2_CASELESS as CASELESS,
        PCRE2_DOLLAR_ENDONLY as DOLLAR_ENDONLY, PCRE2_DOTALL as DOTALL,
        PCRE2_DUPNAMES as DUPNAMES, PCRE2_EXTENDED as EXTENDED, PCRE2_MULTILINE as MULTILINE,
        PCRE2_NO_AUTO_CAPTURE as NO_AUTO_CAPTURE, PCRE2_UCP as UCP, PCRE2_UNGREEDY as UNGREEDY,
        PCRE2_UTF as UTF,
    };
    /// Extra compile option (`pcre2_set_compile_extra_options`): the PHP 8.2
    /// `r` modifier.
    pub const EXTRA_CASELESS_RESTRICT: u32 = pcre2_sys::PCRE2_EXTRA_CASELESS_RESTRICT;
}

/// Match-time option bits accepted by [`Code::exec`] (`pcre2_match`).
pub mod mopt {
    pub use pcre2_sys::{
        PCRE2_ANCHORED as ANCHORED, PCRE2_NOTEMPTY_ATSTART as NOTEMPTY_ATSTART,
        PCRE2_NO_UTF_CHECK as NO_UTF_CHECK,
    };
}

/// The PCRE2 error codes the stdlib maps onto `PREG_*_ERROR`.
pub mod err {
    pub use pcre2_sys::{
        PCRE2_ERROR_BADOFFSET as BADOFFSET, PCRE2_ERROR_BADUTFOFFSET as BADUTFOFFSET,
        PCRE2_ERROR_DEPTHLIMIT as DEPTHLIMIT, PCRE2_ERROR_INTERNAL as INTERNAL,
        PCRE2_ERROR_JIT_STACKLIMIT as JIT_STACKLIMIT, PCRE2_ERROR_MATCHLIMIT as MATCHLIMIT,
        PCRE2_ERROR_NOMATCH as NOMATCH, PCRE2_ERROR_NOMEMORY as NOMEMORY,
        PCRE2_ERROR_UTF8_ERR1 as UTF8_ERR1, PCRE2_ERROR_UTF8_ERR21 as UTF8_ERR21,
    };
}

/// The value PCRE2 stores in the ovector for a group that did not
/// participate in the match.
pub const UNSET: usize = PCRE2_UNSET;

/// `pcre2_config(PCRE2_CONFIG_VERSION)`: the linked library's version string,
/// e.g. `10.47 2025-10-21` (what PHP exposes as `PCRE_VERSION`).
pub fn version() -> String {
    // SAFETY: a NULL `where` argument asks PCRE2 for the buffer size only.
    let len = unsafe { pcre2_config_8(PCRE2_CONFIG_VERSION, ptr::null_mut()) };
    if len <= 0 {
        return String::new();
    }
    let mut buf = vec![0u8; len as usize];
    // SAFETY: `buf` holds exactly the number of code units PCRE2 asked for.
    let written = unsafe { pcre2_config_8(PCRE2_CONFIG_VERSION, buf.as_mut_ptr().cast::<c_void>()) };
    if written <= 0 {
        return String::new();
    }
    // The count includes the terminating NUL.
    buf.truncate((written as usize).saturating_sub(1));
    String::from_utf8_lossy(&buf).into_owned()
}

/// `pcre2_config(PCRE2_CONFIG_JIT)`: whether the library was built with the
/// JIT compiler (`PCRE_JIT_SUPPORT`).
pub fn jit_available() -> bool {
    let mut rc: u32 = 0;
    // SAFETY: `PCRE2_CONFIG_JIT` writes one `uint32_t` through the pointer.
    let code = unsafe { pcre2_config_8(PCRE2_CONFIG_JIT, (&mut rc as *mut u32).cast::<c_void>()) };
    code >= 0 && rc == 1
}

/// `pcre2_get_error_message`: PCRE2's text for a compile or match error code.
pub fn error_message(code: i32) -> String {
    let mut buf = [0u8; 256];
    // SAFETY: `buf` is a writable buffer of the stated length; PCRE2 writes at
    // most that many code units (NUL-terminated).
    let n = unsafe { pcre2_get_error_message_8(code, buf.as_mut_ptr(), buf.len()) };
    if n < 0 {
        return String::from("unknown error");
    }
    String::from_utf8_lossy(&buf[..n as usize]).into_owned()
}

/// A failed `pcre2_compile`.
#[derive(Clone, Debug)]
pub struct CompileError {
    /// The PCRE2 error code.
    pub code: i32,
    /// The byte offset in the pattern PCRE2 reports.
    pub offset: usize,
    /// `pcre2_get_error_message(code)`.
    pub message: String,
}

/// The outcome of one `pcre2_match` call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MatchResult {
    /// Matched; the highest-numbered group that participated is `count - 1`
    /// (so `count` ovector pairs are meaningful).
    Match(usize),
    /// `PCRE2_ERROR_NOMATCH`.
    NoMatch,
    /// Any other negative return (see [`err`]).
    Error(i32),
}

/// A compiled pattern (`pcre2_code`).
///
/// Invariant: `code` is a non-null pointer returned by `pcre2_compile` that
/// is freed exactly once, in `Drop`. The code object is never mutated after
/// construction except by [`Code::jit_compile`], which takes `&mut self`.
pub struct Code {
    code: *mut pcre2_code_8,
    jit: bool,
    capture_count: u32,
    names: Vec<(u32, Vec<u8>)>,
}

impl std::fmt::Debug for Code {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Code")
            .field("jit", &self.jit)
            .field("capture_count", &self.capture_count)
            .field("names", &self.names)
            .finish_non_exhaustive()
    }
}

impl Drop for Code {
    fn drop(&mut self) {
        // SAFETY: `code` came from `pcre2_compile` and is freed only here.
        unsafe { pcre2_code_free_8(self.code) }
    }
}

impl Code {
    /// Compile `pattern` (raw bytes; PCRE2 validates UTF-8 itself when `UTF`
    /// is set) with `options` and `extra_options`.
    pub fn compile(pattern: &[u8], options: u32, extra_options: u32) -> Result<Code, CompileError> {
        let mut error_code: i32 = 0;
        let mut error_offset: usize = 0;
        // SAFETY: a fresh compile context (or NULL on allocation failure —
        // PCRE2 treats NULL as "defaults"), freed after the compile.
        let cctx = unsafe { pcre2_compile_context_create_8(ptr::null_mut()) };
        if !cctx.is_null() && extra_options != 0 {
            // SAFETY: `cctx` is a live compile context.
            unsafe { pcre2_set_compile_extra_options_8(cctx, extra_options) };
        }
        // An empty slice may carry a dangling pointer; PCRE2 never reads past
        // `len`, but hand it a real allocation anyway.
        static EMPTY: [u8; 1] = [0];
        let p = if pattern.is_empty() { EMPTY.as_ptr() } else { pattern.as_ptr() };
        // SAFETY: `p` points at `pattern.len()` readable bytes; the out
        // pointers are valid locals; `cctx` is live or NULL.
        let code = unsafe {
            pcre2_compile_8(p, pattern.len(), options, &mut error_code, &mut error_offset, cctx)
        };
        if !cctx.is_null() {
            // SAFETY: created above, used only by the compile call.
            unsafe { pcre2_compile_context_free_8(cctx) };
        }
        if code.is_null() {
            return Err(CompileError {
                code: error_code,
                offset: error_offset,
                message: error_message(error_code),
            });
        }
        let mut c = Code { code, jit: false, capture_count: 0, names: Vec::new() };
        c.capture_count = c.info_u32(PCRE2_INFO_CAPTURECOUNT);
        c.names = c.read_name_table();
        Ok(c)
    }

    /// `pcre2_pattern_info` for a `uint32_t`-valued item.
    fn info_u32(&self, what: u32) -> u32 {
        let mut v: u32 = 0;
        // SAFETY: `what` names a `uint32_t` item; `v` is a valid out pointer.
        let rc = unsafe { pcre2_pattern_info_8(self.code, what, (&mut v as *mut u32).cast::<c_void>()) };
        if rc == 0 { v } else { 0 }
    }

    /// `pcre2_pattern_info` for a `size_t`-valued item.
    fn info_size(&self, what: u32) -> usize {
        let mut v: usize = 0;
        // SAFETY: `what` names a `size_t` item; `v` is a valid out pointer.
        let rc = unsafe { pcre2_pattern_info_8(self.code, what, (&mut v as *mut usize).cast::<c_void>()) };
        if rc == 0 { v } else { 0 }
    }

    /// Decode `PCRE2_INFO_NAMETABLE` into `(group index, name)` pairs in table
    /// (alphabetical) order. Each entry is two big-endian index bytes, the
    /// NUL-terminated name, and padding to `NAMEENTRYSIZE`.
    fn read_name_table(&self) -> Vec<(u32, Vec<u8>)> {
        let count = self.info_u32(PCRE2_INFO_NAMECOUNT) as usize;
        let entry = self.info_u32(PCRE2_INFO_NAMEENTRYSIZE) as usize;
        if count == 0 || entry < 3 {
            return Vec::new();
        }
        let mut table: *const u8 = ptr::null();
        // SAFETY: `PCRE2_INFO_NAMETABLE` writes a `PCRE2_SPTR` through the pointer.
        let rc = unsafe {
            pcre2_pattern_info_8(self.code, PCRE2_INFO_NAMETABLE, (&mut table as *mut *const u8).cast::<c_void>())
        };
        if rc != 0 || table.is_null() {
            return Vec::new();
        }
        // SAFETY: PCRE2 guarantees the table holds `count * entry` bytes that
        // live as long as the code object (`self`).
        let bytes = unsafe { std::slice::from_raw_parts(table, count * entry) };
        bytes
            .chunks_exact(entry)
            .map(|e| {
                let idx = (u32::from(e[0]) << 8) | u32::from(e[1]);
                let name = &e[2..];
                let end = name.iter().position(|&b| b == 0).unwrap_or(name.len());
                (idx, name[..end].to_vec())
            })
            .collect()
    }

    /// `pcre2_jit_compile(PCRE2_JIT_COMPLETE)`: `Ok(true)` when JIT code was
    /// produced (`PCRE2_INFO_JITSIZE > 0`), `Ok(false)` when the library
    /// silently declined, `Err(code)` on a JIT compile error.
    pub fn jit_compile(&mut self) -> Result<bool, i32> {
        // SAFETY: `code` is live; JIT compilation only attaches data to it.
        let rc = unsafe { pcre2_jit_compile_8(self.code, PCRE2_JIT_COMPLETE) };
        if rc < 0 {
            return Err(rc);
        }
        self.jit = self.info_size(PCRE2_INFO_JITSIZE) > 0;
        Ok(self.jit)
    }

    /// Whether JIT code is attached (matching will run through it).
    pub fn jit(&self) -> bool {
        self.jit
    }

    /// The number of capturing groups, excluding group 0.
    pub fn capture_count(&self) -> u32 {
        self.capture_count
    }

    /// The named groups as `(group index, name)` pairs (table order).
    pub fn names(&self) -> &[(u32, Vec<u8>)] {
        &self.names
    }

    /// The compile options in effect (`PCRE2_INFO_ARGOPTIONS`).
    pub fn options(&self) -> u32 {
        self.info_u32(PCRE2_INFO_ARGOPTIONS)
    }

    /// `pcre2_match` on `subject` from byte offset `start` with match
    /// `options`, under `mctx`'s limits, writing into `md`. `start` may be
    /// any value; PCRE2 reports `BADOFFSET` past the end.
    pub fn exec(&self, md: &mut MatchData, subject: &[u8], start: usize, options: u32, mctx: &MatchContext) -> MatchResult {
        static EMPTY: [u8; 1] = [0];
        let p = if subject.is_empty() { EMPTY.as_ptr() } else { subject.as_ptr() };
        // SAFETY: `p` points at `subject.len()` readable bytes; `md.data` was
        // created from this pattern (or a pattern with at least as many
        // groups); `mctx.ctx` is live. PCRE2 validates UTF-8 unless
        // `NO_UTF_CHECK` is passed, which the caller only does after an
        // earlier checked match of the same subject succeeded.
        let rc = unsafe {
            pcre2_match_8(self.code, p, subject.len(), start, options, md.data, mctx.ctx)
        };
        if rc == PCRE2_ERROR_NOMATCH {
            MatchResult::NoMatch
        } else if rc > 0 {
            MatchResult::Match(rc as usize)
        } else if rc == 0 {
            // The ovector was too small (cannot happen for `MatchData::for_code`).
            MatchResult::Match(md.pairs as usize)
        } else {
            MatchResult::Error(rc)
        }
    }
}

/// A match context (`pcre2_match_context`) carrying the backtrack / depth
/// limits and, once requested, a JIT stack.
///
/// Invariant: `ctx` is non-null and freed once in `Drop`; `jit_stack` is
/// either null or a stack assigned to `ctx`, freed after it.
pub struct MatchContext {
    ctx: *mut pcre2_match_context_8,
    jit_stack: *mut pcre2_jit_stack_8,
}

impl Default for MatchContext {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for MatchContext {
    fn drop(&mut self) {
        // SAFETY: both pointers were created by this type and are freed once.
        unsafe {
            pcre2_match_context_free_8(self.ctx);
            if !self.jit_stack.is_null() {
                pcre2_jit_stack_free_8(self.jit_stack);
            }
        }
    }
}

impl MatchContext {
    /// A context with PCRE2's default limits and no JIT stack.
    ///
    /// # Panics
    /// If PCRE2 cannot allocate the (tiny) context.
    pub fn new() -> MatchContext {
        // SAFETY: allocation with the default general context.
        let ctx = unsafe { pcre2_match_context_create_8(ptr::null_mut()) };
        assert!(!ctx.is_null(), "pcre2: cannot allocate a match context");
        MatchContext { ctx, jit_stack: ptr::null_mut() }
    }

    /// `pcre2_set_match_limit` (PHP's `pcre.backtrack_limit`).
    pub fn set_match_limit(&mut self, limit: u32) {
        // SAFETY: `ctx` is live.
        unsafe { pcre2_set_match_limit_8(self.ctx, limit) };
    }

    /// `pcre2_set_depth_limit` (PHP's `pcre.recursion_limit`).
    pub fn set_depth_limit(&mut self, limit: u32) {
        // SAFETY: `ctx` is live.
        unsafe { pcre2_set_depth_limit_8(self.ctx, limit) };
    }

    /// Create a JIT stack of `min..=max` bytes and assign it to this context
    /// (idempotent). JIT matches without an assigned stack use PCRE2's 32K
    /// machine stack; PHP assigns 32K–192K so `JIT stack limit exhausted`
    /// triggers where it does in php.
    pub fn ensure_jit_stack(&mut self, min: usize, max: usize) {
        if !self.jit_stack.is_null() {
            return;
        }
        // SAFETY: allocation; on failure the stack stays unassigned.
        let stack = unsafe { pcre2_jit_stack_create_8(min, max, ptr::null_mut()) };
        if stack.is_null() {
            return;
        }
        // SAFETY: `ctx` is live and `stack` is a valid JIT stack; a NULL
        // callback means "use this stack directly".
        unsafe { pcre2_jit_stack_assign_8(self.ctx, None, stack.cast::<c_void>()) };
        self.jit_stack = stack;
    }
}

/// A match data block (`pcre2_match_data`) sized for a pattern.
///
/// Invariant: `data` is non-null, freed once in `Drop`, and holds `pairs`
/// ovector pairs.
pub struct MatchData {
    data: *mut pcre2_match_data_8,
    pairs: u32,
}

impl Drop for MatchData {
    fn drop(&mut self) {
        // SAFETY: created by `for_code`, freed once.
        unsafe { pcre2_match_data_free_8(self.data) }
    }
}

impl MatchData {
    /// A block with room for every group of `code` (`capture_count + 1` pairs).
    ///
    /// # Panics
    /// If PCRE2 cannot allocate the block.
    pub fn for_code(code: &Code) -> MatchData {
        // SAFETY: `code.code` is live; default general context.
        let data = unsafe { pcre2_match_data_create_from_pattern_8(code.code, ptr::null_mut()) };
        assert!(!data.is_null(), "pcre2: cannot allocate match data");
        // SAFETY: `data` is live.
        let pairs = unsafe { pcre2_get_ovector_count_8(data) };
        MatchData { data, pairs }
    }

    /// The ovector after a successful [`Code::exec`]: `2 * pairs` offsets,
    /// `[start_i, end_i]` per group, [`UNSET`] for groups that did not
    /// participate (only the first `count` pairs are meaningful).
    pub fn ovector(&self) -> &[usize] {
        // SAFETY: the pointer and count come from the live block; PCRE2
        // initialises every pair on match-data creation and on each match.
        unsafe {
            let p = pcre2_get_ovector_pointer_8(self.data);
            std::slice::from_raw_parts(p, self.pairs as usize * 2)
        }
    }

    /// `pcre2_get_mark`: the `(*MARK:name)` of the last match, if any.
    pub fn mark(&self) -> Option<Vec<u8>> {
        // SAFETY: `data` is live; a non-null mark points at a
        // length-prefixed, NUL-terminated name inside the compiled pattern.
        unsafe {
            let p = pcre2_get_mark_8(self.data);
            if p.is_null() {
                return None;
            }
            // The byte before the name holds its length.
            let len = usize::from(*p.sub(1));
            Some(std::slice::from_raw_parts(p, len).to_vec())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_and_jit_are_reported() {
        let v = version();
        assert!(v.starts_with("10."), "{v}");
        let _ = jit_available();
    }

    #[test]
    fn compile_match_and_groups() {
        let code = Code::compile(b"(?<x>a)(b)?", 0, 0).unwrap();
        assert_eq!(code.capture_count(), 2);
        assert_eq!(code.names(), &[(1, b"x".to_vec())]);
        let mut md = MatchData::for_code(&code);
        let mctx = MatchContext::new();
        assert_eq!(code.exec(&mut md, b"za", 0, 0, &mctx), MatchResult::Match(2));
        assert_eq!(&md.ovector()[..4], &[1, 2, 1, 2]);
        assert_eq!(md.ovector()[4], UNSET);
        assert_eq!(code.exec(&mut md, b"zz", 0, 0, &mctx), MatchResult::NoMatch);
        assert!(matches!(code.exec(&mut md, b"z", 5, 0, &mctx), MatchResult::Error(err::BADOFFSET)));
    }

    #[test]
    fn compile_errors_carry_offsets() {
        let e = Code::compile(b"(", 0, 0).unwrap_err();
        assert_eq!(e.offset, 1);
        assert!(e.message.contains("missing closing parenthesis"), "{}", e.message);
    }

    #[test]
    fn marks_and_limits() {
        let mut code = Code::compile(b"a(*MARK:foo)", 0, 0).unwrap();
        let _ = code.jit_compile();
        let mut md = MatchData::for_code(&code);
        let mut mctx = MatchContext::new();
        mctx.ensure_jit_stack(32 * 1024, 192 * 1024);
        assert_eq!(code.exec(&mut md, b"a", 0, 0, &mctx), MatchResult::Match(1));
        assert_eq!(md.mark(), Some(b"foo".to_vec()));
        let code = Code::compile(b"(?:\\D+|<\\d+>)*[!?]", 0, 0).unwrap();
        let mut md = MatchData::for_code(&code);
        mctx.set_match_limit(10);
        assert_eq!(
            code.exec(&mut md, b"foobar foobar foobar", 0, 0, &mctx),
            MatchResult::Error(err::MATCHLIMIT)
        );
    }
}
