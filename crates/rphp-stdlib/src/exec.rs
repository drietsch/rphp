//! The shell-escaping half of php-src `ext/standard/exec.c`.
//!
//! The process functions themselves (`exec`, `shell_exec`, `system`,
//! `passthru`, `proc_open`) are a later wave; what programs reach for first —
//! and what a static analysis of the Symfony tree finds most of — is the pair
//! that makes a command line safe.

use rphp_runtime::{nf, Ctx, NativeFn, NativeResult};
use rphp_value::{Str, Value};

/// This extension's registry contribution (see `lib.rs`).
pub(crate) static FUNCTIONS: &[NativeFn] = &[
    nf!("escapeshellarg", 1, Some(1), escapeshellarg),
    nf!("escapeshellcmd", 1, Some(1), escapeshellcmd),
];

/// `escapeshellarg(string $arg): string` — one shell word: the whole string
/// in single quotes, with each `'` closed, escaped and reopened.
fn escapeshellarg(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let s = args[0].to_php_bytes();
    let mut out = Vec::with_capacity(s.len() + 2);
    out.push(b'\'');
    for &b in s.iter() {
        if b == b'\'' {
            out.extend_from_slice(b"'\\''");
        } else {
            out.push(b);
        }
    }
    out.push(b'\'');
    Ok(Value::Str(Str::from_vec(out)))
}

/// `escapeshellcmd(string $command): string` — escape the metacharacters of a
/// *command line*, which is a weaker guarantee than quoting an argument.
///
/// Two rules are easy to miss. A quote is escaped only when it has no partner
/// later in the string, so `"a&b"` keeps its quotes (and still escapes the
/// `&` between them) while a lone `"` is escaped. And a `0xFF` byte is
/// **dropped**, not escaped — php removes it outright.
fn escapeshellcmd(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let s = args[0].to_php_bytes();
    let mut out = Vec::with_capacity(s.len());
    for (i, &b) in s.iter().enumerate() {
        match b {
            0xFF => {}
            b'"' | b'\'' => {
                // Paired: leave both alone. php looks for the next identical
                // quote anywhere in the rest of the string.
                if s[i + 1..].contains(&b) || out.contains(&b) && paired_before(&s[..i], b) {
                    out.push(b);
                } else {
                    out.push(b'\\');
                    out.push(b);
                }
            }
            b'#' | b'&' | b';' | b'`' | b'|' | b'*' | b'?' | b'~' | b'<' | b'>' | b'^' | b'('
            | b')' | b'[' | b']' | b'{' | b'}' | b'$' | b'\\' | 0x0A => {
                out.push(b'\\');
                out.push(b);
            }
            _ => out.push(b),
        }
    }
    Ok(Value::Str(Str::from_vec(out)))
}

/// Whether `q` already opened a pair in the part of the string behind us, so
/// this one closes it rather than dangling.
fn paired_before(before: &[u8], q: u8) -> bool {
    before.iter().filter(|&&b| b == q).count() % 2 == 1
}

/// No constants: `escapeshellcmd`'s character set is not exposed.
pub(crate) fn register_constants(_r: &mut rphp_runtime::Registry) {}
