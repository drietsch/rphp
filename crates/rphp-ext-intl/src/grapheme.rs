//! The `grapheme_*` functions: extended grapheme clusters (UAX #29,
//! ICU4X's segmenter) behind php's byte-oriented API — an ASCII string
//! takes the byte path as in ext/intl, positions count clusters, a
//! negative start walks from the end, `grapheme_extract` returns bytes and
//! reports where the next call should start.
//!
//! The search functions mirror ICU's collation search as php uses it:
//! canonically equivalent text matches (both sides NFD), a match must
//! start and end on cluster boundaries, `stripos`/`strripos` fold case.

use icu::casemap::CaseMapperBorrowed;
use icu::normalizer::DecomposingNormalizerBorrowed;
use icu::segmenter::GraphemeClusterSegmenter;
use rphp_runtime::{Ctx, NativeResult, Registry, Unwind};
use rphp_value::{Array, Value};

use crate::shape::{register_functions, FnImpl};
use crate::state::{self, U_ILLEGAL_ARGUMENT_ERROR, U_INVALID_CHAR_FOUND};
use crate::{generated, opt_arg, str_arg};

const EXTR_COUNT: i64 = 0;
const EXTR_MAXBYTES: i64 = 1;
const EXTR_MAXCHARS: i64 = 2;

/// php's `grapheme_ascii_check`: pure ASCII without a `\r\n` (which is
/// one cluster).
fn is_ascii(s: &[u8]) -> bool {
    s.iter().all(|b| *b < 0x80) && !s.windows(2).any(|w| w == b"\r\n")
}

/// The cluster boundaries of `s` (byte offsets, `0` first, `len` last).
fn boundaries(s: &str) -> Vec<usize> {
    let seg = GraphemeClusterSegmenter::new();
    let mut b: Vec<usize> = seg.segment_str(s).collect();
    if b.first() != Some(&0) {
        b.insert(0, 0);
    }
    if b.last() != Some(&s.len()) {
        b.push(s.len());
    }
    b
}

/// The clusters of `s` as byte ranges.
fn clusters(s: &str) -> Vec<(usize, usize)> {
    let b = boundaries(s);
    b.windows(2).map(|w| (w[0], w[1])).collect()
}

fn utf16_len(s: &str) -> i64 {
    s.encode_utf16().count() as i64
}

fn invalid_input(ctx: &mut Ctx, msg: &str) -> Result<(), Unwind> {
    let who = ctx.active_function_name();
    state::set_global(ctx, &who, U_INVALID_CHAR_FOUND, msg)
}

fn utf8_arg<'a>(ctx: &mut Ctx, bytes: &'a [u8], msg: &str) -> Result<Option<&'a str>, Unwind> {
    match std::str::from_utf8(bytes) {
        Ok(s) => Ok(Some(s)),
        Err(_) => {
            invalid_input(ctx, msg)?;
            Ok(None)
        }
    }
}

fn strlen(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let s = str_arg(args, 0);
    if is_ascii(&s) {
        return Ok(Value::Int(s.len() as i64));
    }
    let Some(text) = utf8_arg(ctx, &s, "Error converting input string to UTF-16")? else {
        return Ok(Value::Null);
    };
    Ok(Value::Int(clusters(text).len() as i64))
}

// ---- search ---------------------------------------------------------------------------

/// The comparable form of a cluster: NFD, case-folded when asked.
fn key(s: &str, fold: bool) -> String {
    let nfd = DecomposingNormalizerBorrowed::new_nfd().normalize(s);
    if fold {
        CaseMapperBorrowed::new().fold_string(&nfd).into_owned()
    } else {
        nfd.into_owned()
    }
}

/// The cluster index the offset names: `offset` clusters from the start,
/// or from the end when negative; `None` when outside the string.
fn offset_cluster(count: usize, offset: i64) -> Option<usize> {
    if offset >= 0 {
        (offset as usize <= count).then_some(offset as usize)
    } else {
        let back = offset.unsigned_abs() as usize;
        (back <= count).then(|| count - back)
    }
}

fn offset_error(ctx: &Ctx) -> Unwind {
    let who = ctx.active_function_name();
    Unwind::value_error(format!("{who}(): Argument #3 ($offset) must be contained in argument #1 ($haystack)"))
}

/// The search behind `grapheme_str[r][i]pos` and `grapheme_str[i]str`:
/// the cluster index of the match, and its byte range.
fn search(
    ctx: &mut Ctx,
    haystack: &[u8],
    needle: &[u8],
    offset: i64,
    ignore_case: bool,
    last: bool,
) -> Result<Option<(i64, usize, usize)>, Unwind> {
    // The byte path: php's own `strpos` family on ASCII haystacks.
    if is_ascii(haystack) && offset >= 0 || is_ascii(haystack) && !last && offset < 0 {
        if offset.unsigned_abs() as usize > haystack.len() {
            return Err(offset_error(ctx));
        }
        if !is_ascii(needle) && ignore_case {
            return Ok(None);
        }
        let fold = |b: &[u8]| -> Vec<u8> { if ignore_case { b.to_ascii_lowercase() } else { b.to_vec() } };
        let h = fold(haystack);
        let n = fold(needle);
        let start = if offset >= 0 { offset as usize } else { haystack.len() - offset.unsigned_abs() as usize };
        let found = if last {
            if n.is_empty() {
                Some(haystack.len())
            } else {
                h[start..].windows(n.len()).rposition(|w| w == n.as_slice()).map(|p| p + start)
            }
        } else if n.is_empty() {
            Some(start)
        } else {
            h[start..].windows(n.len()).position(|w| w == n.as_slice()).map(|p| p + start)
        };
        return Ok(found.map(|p| (p as i64, p, p + needle.len())));
    }
    let Some(h) = utf8_arg(ctx, haystack, "Error converting input string to UTF-16")? else {
        return Ok(None);
    };
    let Some(n) = utf8_arg(ctx, needle, "Error converting needle string to UTF-16")? else {
        return Ok(None);
    };
    let hc = clusters(h);
    let Some(start) = offset_cluster(hc.len(), offset) else {
        return Err(offset_error(ctx));
    };
    if n.is_empty() {
        // php's quirk: the last position of an empty needle is the UTF-16
        // length of the haystack.
        return Ok(Some(if last && offset >= 0 { (utf16_len(h), h.len(), h.len()) } else { (start as i64, hc[..start].last().map_or(0, |c| c.1), 0) }));
    }
    let hk: Vec<String> = hc.iter().map(|(a, b)| key(&h[*a..*b], ignore_case)).collect();
    let nk: Vec<String> = clusters(n).iter().map(|(a, b)| key(&n[*a..*b], ignore_case)).collect();
    if nk.len() > hk.len() {
        return Ok(None);
    }
    let matches_at = |i: usize| i + nk.len() <= hk.len() && hk[i..i + nk.len()] == nk[..];
    let found = if last {
        if offset >= 0 {
            (start..=hk.len() - nk.len()).rev().find(|&i| matches_at(i))
        } else {
            (0..=start.min(hk.len() - nk.len())).rev().find(|&i| matches_at(i))
        }
    } else {
        (start..=hk.len() - nk.len()).find(|&i| matches_at(i))
    };
    Ok(found.map(|i| (i as i64, hc[i].0, hc[i + nk.len() - 1].1)))
}

fn strpos_family(ctx: &mut Ctx, args: &mut [Value], ignore_case: bool, last: bool) -> NativeResult {
    let haystack = str_arg(args, 0);
    let needle = str_arg(args, 1);
    let offset = args.get(2).map_or(0, Value::to_int);
    match search(ctx, &haystack, &needle, offset, ignore_case, last)? {
        Some((pos, _, _)) => Ok(Value::Int(pos)),
        None => Ok(Value::Bool(false)),
    }
}

fn strpos(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    strpos_family(ctx, args, false, false)
}

fn stripos(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    strpos_family(ctx, args, true, false)
}

fn strrpos(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    strpos_family(ctx, args, false, true)
}

fn strripos(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    strpos_family(ctx, args, true, true)
}

fn strstr_family(ctx: &mut Ctx, args: &mut [Value], ignore_case: bool) -> NativeResult {
    let haystack = str_arg(args, 0);
    let needle = str_arg(args, 1);
    let before = args.get(2).is_some_and(Value::to_bool);
    match search(ctx, &haystack, &needle, 0, ignore_case, false)? {
        Some((_, from, _)) => {
            if before {
                Ok(Value::string(&haystack[..from]))
            } else {
                Ok(Value::string(&haystack[from..]))
            }
        }
        None => Ok(Value::Bool(false)),
    }
}

fn strstr(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    strstr_family(ctx, args, false)
}

fn stristr(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    strstr_family(ctx, args, true)
}

// ---- substr ---------------------------------------------------------------------------

fn substr(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let s = str_arg(args, 0);
    let start = args.get(1).map_or(0, Value::to_int);
    let length = opt_arg(args, 2).map(Value::to_int);
    let who = ctx.active_function_name();
    if !(i64::from(i32::MIN)..=i64::from(i32::MAX)).contains(&start) {
        return Err(Unwind::value_error(format!("{who}(): Argument #2 ($start) is too large")));
    }
    if let Some(l) = length {
        if !(i64::from(i32::MIN)..=i64::from(i32::MAX)).contains(&l) {
            return Err(Unwind::value_error(format!("{who}(): Argument #3 ($length) is too large")));
        }
    }
    if is_ascii(&s) {
        // php's `grapheme_substr_ascii`: a start past either end clamps, a
        // negative length stops that many bytes before the end.
        let n = s.len() as i64;
        let from = if start < 0 { (n + start).max(0) } else { start.min(n) };
        let l = match length {
            None => n - from,
            Some(l) if l < 0 => (n - from + l).max(0),
            Some(l) => l.min(n - from),
        };
        return Ok(Value::string(&s[from as usize..(from + l) as usize]));
    }
    let Some(text) = utf8_arg(ctx, &s, "Error converting input string to UTF-16")? else {
        return Ok(Value::Bool(false));
    };
    let cl = clusters(text);
    let count = cl.len() as i64;
    // the start: walked forward or backward, clamped the way php's loop ends
    let from = if start >= 0 {
        if start > count {
            return Ok(Value::string(b""));
        }
        start
    } else {
        (count + start).max(0)
    };
    let to = match length {
        None => count,
        Some(l) if l >= s.len() as i64 => count,
        Some(0) => return Ok(Value::string(b"")),
        Some(l) if l < 0 => {
            let e = count + l;
            if e < 0 {
                return Ok(Value::string(b""));
            }
            e
        }
        Some(l) => (from + l).min(count),
    };
    if from >= to {
        return Ok(Value::string(b""));
    }
    let a = cl[from as usize].0;
    let b = cl[to as usize - 1].1;
    Ok(Value::string(text[a..b].as_bytes()))
}

// ---- extract / split ------------------------------------------------------------------

fn extract(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let s = str_arg(args, 0);
    let size = args.get(1).map_or(0, Value::to_int);
    let kind = args.get(2).map_or(EXTR_COUNT, Value::to_int);
    let mut start = args.get(3).map_or(0, Value::to_int);
    let who = ctx.active_function_name();
    if start < 0 {
        start += s.len() as i64;
    }
    // `$next` is by reference: the runtime writes `args[4]` back.
    let has_next = args.len() > 4;
    let set_next = |args: &mut [Value], v: i64| {
        if has_next {
            args[4] = Value::Int(v);
        }
    };
    set_next(args, start);
    if !(EXTR_COUNT..=EXTR_MAXCHARS).contains(&kind) {
        return Err(Unwind::value_error(format!(
            "{who}(): Argument #3 ($type) must be one of GRAPHEME_EXTR_COUNT, GRAPHEME_EXTR_MAXBYTES, or GRAPHEME_EXTR_MAXCHARS"
        )));
    }
    if start < 0 || start as usize >= s.len() {
        state::set_global(ctx, &who, U_ILLEGAL_ARGUMENT_ERROR, "start not contained in string")?;
        return Ok(Value::Bool(false));
    }
    if size < 0 {
        return Err(Unwind::value_error(format!("{who}(): Argument #2 ($size) must be greater than or equal to 0")));
    }
    if size > i64::from(i32::MAX) {
        return Err(Unwind::value_error(format!("{who}(): Argument #2 ($size) is too large")));
    }
    if size == 0 {
        return Ok(Value::string(b""));
    }
    let mut at = start as usize;
    // in the middle of a character: forward to the next lead byte
    while at < s.len() && (s[at] & 0xC0) == 0x80 {
        at += 1;
        if at >= s.len() {
            state::set_global(ctx, &who, U_ILLEGAL_ARGUMENT_ERROR, "grapheme_extract: invalid input string")?;
            return Ok(Value::Bool(false));
        }
    }
    let rest = &s[at..];
    let probe = (size as usize + 1).min(rest.len());
    if is_ascii(&rest[..probe]) {
        let n = (size as usize).min(rest.len());
        set_next(args, (at + n) as i64);
        return Ok(Value::string(&rest[..n]));
    }
    let text = match std::str::from_utf8(rest) {
        Ok(t) => t,
        Err(e) => {
            let valid = e.valid_up_to();
            if valid == 0 {
                state::set_global(ctx, &who, U_ILLEGAL_ARGUMENT_ERROR, "Error opening UTF-8 text")?;
                return Ok(Value::Bool(false));
            }
            std::str::from_utf8(&rest[..valid]).unwrap_or("")
        }
    };
    let cl = clusters(text);
    let mut end = 0usize;
    match kind {
        EXTR_COUNT => {
            for (i, c) in cl.iter().enumerate() {
                if i as i64 >= size {
                    break;
                }
                end = c.1;
            }
        }
        EXTR_MAXBYTES => {
            for c in &cl {
                if c.1 as i64 > size {
                    break;
                }
                end = c.1;
            }
        }
        _ => {
            let mut chars = 0i64;
            for c in &cl {
                chars += text[c.0..c.1].chars().count() as i64;
                if chars > size {
                    break;
                }
                end = c.1;
            }
        }
    }
    set_next(args, (at + end) as i64);
    Ok(Value::string(&rest[..end]))
}

fn str_split(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let s = str_arg(args, 0);
    let len = args.get(1).map_or(1, Value::to_int);
    if len <= 0 || len > i64::from(u32::MAX / 4) {
        let who = ctx.active_function_name();
        return Err(Unwind::value_error(format!(
            "{who}(): Argument #2 ($length) must be greater than 0 and less than or equal to {}",
            u32::MAX / 4
        )));
    }
    let mut out = Array::new();
    if s.is_empty() {
        return Ok(Value::Array(out));
    }
    let Some(text) = utf8_arg(ctx, &s, "Error opening UTF-8 text")? else {
        return Ok(Value::Bool(false));
    };
    let cl = clusters(text);
    for chunk in cl.chunks(len as usize) {
        let a = chunk[0].0;
        let b = chunk[chunk.len() - 1].1;
        out.push(Value::string(text[a..b].as_bytes()));
    }
    Ok(Value::Array(out))
}

fn levenshtein(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let s1 = str_arg(args, 0);
    let s2 = str_arg(args, 1);
    let ins = args.get(2).map_or(1, Value::to_int);
    let rep = args.get(3).map_or(1, Value::to_int);
    let del = args.get(4).map_or(1, Value::to_int);
    let who = ctx.active_function_name();
    for (i, cost) in [(3, ins), (4, rep), (5, del)] {
        if cost <= 0 || cost > i64::from(u32::MAX / 4) {
            return Err(Unwind::value_error(format!(
                "{who}(): Argument #{i} (${}) must be greater than 0 and less than or equal to {}",
                match i {
                    3 => "insertion_cost",
                    4 => "replacement_cost",
                    _ => "deletion_cost",
                },
                u32::MAX / 4
            )));
        }
    }
    let Some(t1) = utf8_arg(ctx, &s1, "Error converting input string to UTF-16")? else {
        return Ok(Value::Bool(false));
    };
    let Some(t2) = utf8_arg(ctx, &s2, "Error converting input string to UTF-16")? else {
        return Ok(Value::Bool(false));
    };
    let a: Vec<String> = clusters(t1).iter().map(|(x, y)| key(&t1[*x..*y], false)).collect();
    let b: Vec<String> = clusters(t2).iter().map(|(x, y)| key(&t2[*x..*y], false)).collect();
    if a.is_empty() {
        return Ok(Value::Int(b.len() as i64 * ins));
    }
    if b.is_empty() {
        return Ok(Value::Int(a.len() as i64 * del));
    }
    let mut prev: Vec<i64> = (0..=b.len() as i64).map(|j| j * ins).collect();
    let mut cur = vec![0i64; b.len() + 1];
    for (i, ca) in a.iter().enumerate() {
        cur[0] = (i as i64 + 1) * del;
        for (j, cb) in b.iter().enumerate() {
            let c0 = prev[j] + if ca == cb { 0 } else { rep };
            let c1 = prev[j + 1] + del;
            let c2 = cur[j] + ins;
            cur[j + 1] = c0.min(c1).min(c2);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    Ok(Value::Int(prev[b.len()]))
}

static FUNCTIONS: &[FnImpl] = &[
    ("grapheme_strlen", strlen),
    ("grapheme_strpos", strpos),
    ("grapheme_stripos", stripos),
    ("grapheme_strrpos", strrpos),
    ("grapheme_strripos", strripos),
    ("grapheme_substr", substr),
    ("grapheme_strstr", strstr),
    ("grapheme_stristr", stristr),
    ("grapheme_extract", extract),
    ("grapheme_str_split", str_split),
    ("grapheme_levenshtein", levenshtein),
];

pub fn register(r: &mut Registry) {
    register_functions(r, generated::arginfo::FUNCTIONS, FUNCTIONS);
}
