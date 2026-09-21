//! The second half of php-src `ext/standard/array.c` (the first lives in
//! `arrays.rs`): the `zend_sort` port and the `SORT_*` comparators every
//! sort shares, natural-order sorts, `array_multisort`, the internal-pointer
//! functions, `array_walk`, the keyed / callback set operations,
//! recursive merge/replace, `array_find`/`array_any`/`array_all`, and the
//! `SORT_*`/`COUNT_*`/`ARRAY_FILTER_*`/`EXTR_*`/`CASE_*` constants.
use std::cell::Cell;
use std::cmp::Ordering;

use rphp_value::{Array, ArrayKey, Value};

use rphp_runtime::{nf, nf_ref, Ctx, NativeFn, NativeResult, Registry, Unwind};

use crate::string2::strnatcmp_bytes;

/// Functions this module provides.
pub(crate) static FUNCTIONS: &[NativeFn] = &[
    nf!("array_change_key_case", 1, Some(2), array_change_key_case),
    nf!("key_exists", 2, Some(2), key_exists),
    nf!("array_diff_key", 1, None, array_diff_key),
    nf!("array_diff_assoc", 1, None, array_diff_assoc),
    nf!("array_diff_ukey", 1, None, array_diff_ukey),
    nf!("array_diff_uassoc", 1, None, array_diff_uassoc),
    nf!("array_udiff", 1, None, array_udiff),
    nf!("array_udiff_assoc", 1, None, array_udiff_assoc),
    nf!("array_udiff_uassoc", 1, None, array_udiff_uassoc),
    nf!("array_intersect_key", 1, None, array_intersect_key),
    nf!("array_intersect_assoc", 1, None, array_intersect_assoc),
    nf!("array_intersect_ukey", 1, None, array_intersect_ukey),
    nf!("array_intersect_uassoc", 1, None, array_intersect_uassoc),
    nf!("array_uintersect", 1, None, array_uintersect),
    nf!("array_uintersect_assoc", 1, None, array_uintersect_assoc),
    nf!("array_uintersect_uassoc", 1, None, array_uintersect_uassoc),
    nf!("array_merge_recursive", 0, None, array_merge_recursive),
    nf!("array_replace_recursive", 1, None, array_replace_recursive),
    nf!("array_first", 1, Some(1), array_first),
    nf!("array_last", 1, Some(1), array_last),
    nf!("array_find", 2, Some(2), array_find),
    nf!("array_find_key", 2, Some(2), array_find_key),
    nf!("array_any", 2, Some(2), array_any),
    nf!("array_all", 2, Some(2), array_all),
    nf!("current", 1, Some(1), current),
    nf!("pos", 1, Some(1), current),
    nf!("key", 1, Some(1), key),
    nf_ref!("next", 1, Some(1), 0b1, next),
    nf_ref!("prev", 1, Some(1), 0b1, prev),
    nf_ref!("reset", 1, Some(1), 0b1, reset),
    nf_ref!("end", 1, Some(1), 0b1, end),
    nf_ref!("natsort", 1, Some(1), 0b1, natsort),
    nf_ref!("natcasesort", 1, Some(1), 0b1, natcasesort),
    // `array_walk` itself is registered by `funcs.rs` (engine by-ref work).
    nf_ref!("array_walk_recursive", 2, Some(3), 0b1, array_walk_recursive),
    // `array_multisort(&$array, &...$rest)`: every position is by reference
    // (php's "prefer ref" — plain values are accepted too).
    nf_ref!("array_multisort", 1, None, 0xFFFF_FFFF, array_multisort),
];

/// The array-extension constants (values from `manifest/php-8.5.0/constants.json`).
pub(crate) fn register_constants(r: &mut Registry) {
    for (name, v) in [
        ("EXTR_OVERWRITE", 0),
        ("EXTR_SKIP", 1),
        ("EXTR_PREFIX_SAME", 2),
        ("EXTR_PREFIX_ALL", 3),
        ("EXTR_PREFIX_INVALID", 4),
        ("EXTR_PREFIX_IF_EXISTS", 5),
        ("EXTR_IF_EXISTS", 6),
        ("EXTR_REFS", 256),
        ("SORT_ASC", 4),
        ("SORT_DESC", 3),
        ("SORT_REGULAR", 0),
        ("SORT_NUMERIC", 1),
        ("SORT_STRING", 2),
        ("SORT_LOCALE_STRING", 5),
        ("SORT_NATURAL", 6),
        ("SORT_FLAG_CASE", 8),
        ("CASE_LOWER", 0),
        ("CASE_UPPER", 1),
        ("COUNT_NORMAL", 0),
        ("COUNT_RECURSIVE", 1),
        ("ARRAY_FILTER_USE_BOTH", 1),
        ("ARRAY_FILTER_USE_KEY", 2),
    ] {
        r.constant(name, Value::Int(v));
    }
}

pub(crate) const SORT_REGULAR: i64 = 0;
pub(crate) const SORT_NUMERIC: i64 = 1;
pub(crate) const SORT_STRING: i64 = 2;
pub(crate) const SORT_LOCALE_STRING: i64 = 5;
pub(crate) const SORT_NATURAL: i64 = 6;
pub(crate) const SORT_FLAG_CASE: i64 = 8;

// ---- zend_sort -------------------------------------------------------------------
//
// php sorts with its own hybrid insertion sort / quicksort. Every comparator
// here is a total order (ties fall back to the original position), so any
// correct sort would agree on well-behaved input; the port keeps the exact
// element order php produces for inconsistent comparators (mixed-type
// SORT_REGULAR, user callbacks) too.

fn gt<T>(cmp: &mut impl FnMut(&T, &T) -> Ordering, a: &T, b: &T) -> bool {
    cmp(a, b) == Ordering::Greater
}

fn sort2<T>(v: &mut [T], a: usize, b: usize, cmp: &mut impl FnMut(&T, &T) -> Ordering) {
    if gt(cmp, &v[a], &v[b]) {
        v.swap(a, b);
    }
}

fn sort3<T>(v: &mut [T], a: usize, b: usize, c: usize, cmp: &mut impl FnMut(&T, &T) -> Ordering) {
    if !gt(cmp, &v[a], &v[b]) {
        if !gt(cmp, &v[b], &v[c]) {
            return;
        }
        v.swap(b, c);
        if gt(cmp, &v[a], &v[b]) {
            v.swap(a, b);
        }
        return;
    }
    if !gt(cmp, &v[c], &v[b]) {
        v.swap(a, c);
        return;
    }
    v.swap(a, b);
    if gt(cmp, &v[b], &v[c]) {
        v.swap(b, c);
    }
}

fn sort4<T>(v: &mut [T], a: usize, b: usize, c: usize, d: usize, cmp: &mut impl FnMut(&T, &T) -> Ordering) {
    sort3(v, a, b, c, cmp);
    if gt(cmp, &v[c], &v[d]) {
        v.swap(c, d);
        if gt(cmp, &v[b], &v[c]) {
            v.swap(b, c);
            if gt(cmp, &v[a], &v[b]) {
                v.swap(a, b);
            }
        }
    }
}

fn sort5<T>(v: &mut [T], a: usize, b: usize, c: usize, d: usize, e: usize, cmp: &mut impl FnMut(&T, &T) -> Ordering) {
    sort4(v, a, b, c, d, cmp);
    if gt(cmp, &v[d], &v[e]) {
        v.swap(d, e);
        if gt(cmp, &v[c], &v[d]) {
            v.swap(c, d);
            if gt(cmp, &v[b], &v[c]) {
                v.swap(b, c);
                if gt(cmp, &v[a], &v[b]) {
                    v.swap(a, b);
                }
            }
        }
    }
}

/// php's `zend_insert_sort` on `v[lo..lo+n]`.
fn insert_sort<T>(v: &mut [T], lo: usize, n: usize, cmp: &mut impl FnMut(&T, &T) -> Ordering) {
    match n {
        0 | 1 => {}
        2 => sort2(v, lo, lo + 1, cmp),
        3 => sort3(v, lo, lo + 1, lo + 2, cmp),
        4 => sort4(v, lo, lo + 1, lo + 2, lo + 3, cmp),
        5 => sort5(v, lo, lo + 1, lo + 2, lo + 3, lo + 4, cmp),
        _ => {
            let start = lo;
            let end = lo + n;
            let sentry = start + 6;
            let mut i = start + 1;
            while i < sentry {
                let mut j = i - 1;
                if !gt(cmp, &v[j], &v[i]) {
                    i += 1;
                    continue;
                }
                while j != start {
                    j -= 1;
                    if !gt(cmp, &v[j], &v[i]) {
                        j += 1;
                        break;
                    }
                }
                let mut k = i;
                while k > j {
                    v.swap(k, k - 1);
                    k -= 1;
                }
                i += 1;
            }
            let mut i = sentry;
            while i < end {
                let mut j = i - 1;
                if !gt(cmp, &v[j], &v[i]) {
                    i += 1;
                    continue;
                }
                loop {
                    j -= 2;
                    if !gt(cmp, &v[j], &v[i]) {
                        j += 1;
                        if !gt(cmp, &v[j], &v[i]) {
                            j += 1;
                        }
                        break;
                    }
                    if j == start {
                        break;
                    }
                    if j == start + 1 {
                        j -= 1;
                        if gt(cmp, &v[i], &v[j]) {
                            j += 1;
                        }
                        break;
                    }
                }
                let mut k = i;
                while k > j {
                    v.swap(k, k - 1);
                    k -= 1;
                }
                i += 1;
            }
        }
    }
}

/// php's `zend_sort` (hybrid: insertion sort up to 16 elements, otherwise a
/// median-pivot quicksort recursing into the smaller side).
pub(crate) fn zend_sort<T>(v: &mut [T], cmp: &mut impl FnMut(&T, &T) -> Ordering) {
    let mut lo = 0usize;
    let mut n = v.len();
    loop {
        if n <= 16 {
            insert_sort(v, lo, n, cmp);
            return;
        }
        let start = lo;
        let end = lo + n;
        let offset = n >> 1;
        let pivot = start + offset;
        if n >> 10 != 0 {
            let delta = offset >> 1;
            sort5(v, start, start + delta, pivot, pivot + delta, end - 1, cmp);
        } else {
            sort3(v, start, pivot, end - 1, cmp);
        }
        v.swap(start + 1, pivot);
        let pivot = start + 1;
        let mut i = pivot + 1;
        let mut j = end - 1;
        'partition: loop {
            while gt(cmp, &v[pivot], &v[i]) {
                i += 1;
                if i == j {
                    break 'partition;
                }
            }
            j -= 1;
            if j == i {
                break 'partition;
            }
            while gt(cmp, &v[j], &v[pivot]) {
                j -= 1;
                if j == i {
                    break 'partition;
                }
            }
            v.swap(i, j);
            i += 1;
            if i == j {
                break 'partition;
            }
        }
        v.swap(pivot, i - 1);
        let left_n = (i - 1) - start;
        let right_n = end - i;
        if left_n < right_n {
            zend_sort(&mut v[start..i - 1], cmp);
            lo = i;
            n = right_n;
        } else {
            zend_sort(&mut v[i..end], cmp);
            lo = start;
            n = left_n;
        }
    }
}

// ---- comparators -------------------------------------------------------------------

fn ord(i: i64) -> Ordering {
    i.cmp(&0)
}

/// `(string)$v` with php's "Array to string conversion" warning.
pub(crate) fn sort_string_of(ctx: &mut Ctx, v: &Value) -> Result<Vec<u8>, Unwind> {
    // The engine's `(string)`: `__toString()` for an object (an `Error`
    // without one), the `Array to string conversion` warning for an array.
    Ok(ctx.to_string(v)?.as_bytes().to_vec())
}

/// php's `php_get_data_compare_func` family, applied to two values.
pub(crate) fn compare_values(ctx: &mut Ctx, flags: i64, a: &Value, b: &Value) -> Result<Ordering, Unwind> {
    let ci = flags & SORT_FLAG_CASE != 0;
    Ok(match flags & !SORT_FLAG_CASE {
        SORT_NUMERIC => a.to_float().partial_cmp(&b.to_float()).unwrap_or(Ordering::Equal),
        SORT_STRING | SORT_LOCALE_STRING => {
            let (x, y) = (sort_string_of(ctx, a)?, sort_string_of(ctx, b)?);
            if ci && flags & !SORT_FLAG_CASE == SORT_STRING {
                x.to_ascii_lowercase().cmp(&y.to_ascii_lowercase())
            } else {
                x.cmp(&y)
            }
        }
        SORT_NATURAL => {
            let (x, y) = (sort_string_of(ctx, a)?, sort_string_of(ctx, b)?);
            ord(strnatcmp_bytes(&x, &y, ci))
        }
        _ => ord(a.spaceship(b)),
    })
}

/// php's `php_get_key_compare_func` family, applied to two keys.
pub(crate) fn compare_keys(flags: i64, a: &ArrayKey, b: &ArrayKey) -> Ordering {
    let ci = flags & SORT_FLAG_CASE != 0;
    let key_bytes = |k: &ArrayKey| -> Vec<u8> {
        match k {
            ArrayKey::Int(i) => i.to_string().into_bytes(),
            ArrayKey::Str(s) => s.to_vec(),
        }
    };
    match flags & !SORT_FLAG_CASE {
        SORT_NUMERIC => {
            let f = |k: &ArrayKey| -> f64 {
                match k {
                    ArrayKey::Int(i) => *i as f64,
                    ArrayKey::Str(s) => Value::string(s).to_float(),
                }
            };
            f(a).partial_cmp(&f(b)).unwrap_or(Ordering::Equal)
        }
        SORT_STRING | SORT_LOCALE_STRING => {
            let (x, y) = (key_bytes(a), key_bytes(b));
            if ci && flags & !SORT_FLAG_CASE == SORT_STRING {
                x.to_ascii_lowercase().cmp(&y.to_ascii_lowercase())
            } else {
                x.cmp(&y)
            }
        }
        SORT_NATURAL => ord(strnatcmp_bytes(&key_bytes(a), &key_bytes(b), ci)),
        _ => match (a, b) {
            (ArrayKey::Int(x), ArrayKey::Int(y)) => x.cmp(y),
            _ => ord(a.to_value().spaceship(&b.to_value())),
        },
    }
}

thread_local! {
    /// php emits the "returning bool from a comparison function" deprecation
    /// once per sorting call; the flag is reset by [`begin_user_sort`].
    static BOOL_CMP_DEPRECATED: Cell<bool> = const { Cell::new(false) };
}

/// Reset the per-call deprecation flag before a sort that takes a callback.
pub(crate) fn begin_user_sort() {
    BOOL_CMP_DEPRECATED.with(|c| c.set(false));
}

/// Invoke a user comparison callback the way php's sorts do: an integer
/// result's sign, with the deprecated `bool` return retried with swapped
/// operands (so `fn($a, $b) => $a > $b` keeps working). `func` prefixes the
/// deprecation.
pub(crate) fn user_compare(ctx: &mut Ctx, func: &str, cb: &Value, a: &Value, b: &Value) -> Result<Ordering, Unwind> {
    let r = ctx.call_value(cb, &[a.clone(), b.clone()])?;
    if let Value::Bool(flag) = r {
        if !BOOL_CMP_DEPRECATED.with(|c| c.replace(true)) {
            ctx.deprecated(&format!("{func}(): Returning bool from comparison function is deprecated, return an integer less than, equal to, or greater than zero"))?;
        }
        if flag {
            return Ok(Ordering::Greater);
        }
        let r2 = ctx.call_value(cb, &[b.clone(), a.clone()])?;
        return Ok(ord(-r2.to_int().signum()));
    }
    Ok(ord(r.to_int().signum()))
}

/// Sort owned `(key, value)` entries with a fallible comparator, ties broken
/// by original position (php's stable sort). The first error aborts the sort.
pub(crate) fn sort_entries(
    entries: &mut Vec<(ArrayKey, Value)>,
    mut cmp: impl FnMut(&(ArrayKey, Value), &(ArrayKey, Value)) -> Result<Ordering, Unwind>,
) -> Result<(), Unwind> {
    let mut indexed: Vec<(usize, (ArrayKey, Value))> = std::mem::take(entries).into_iter().enumerate().collect();
    let mut err: Option<Unwind> = None;
    zend_sort(&mut indexed, &mut |a, b| {
        if err.is_some() {
            return Ordering::Equal;
        }
        match cmp(&a.1, &b.1) {
            Ok(Ordering::Equal) => a.0.cmp(&b.0),
            Ok(o) => o,
            Err(e) => {
                err = Some(e);
                Ordering::Equal
            }
        }
    });
    *entries = indexed.into_iter().map(|(_, e)| e).collect();
    match err {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

// ---- helpers -----------------------------------------------------------------------

fn want_array<'a>(func: &str, v: &'a Value) -> Result<&'a Array, Unwind> {
    match v {
        Value::Array(a) => Ok(a),
        other => Err(Unwind::type_error(format!(
            "{func}(): Argument #1 ($array) must be of type array, {} given",
            rphp_runtime::value_name(&other)
        ))),
    }
}

fn want_array_n<'a>(func: &str, n: usize, v: &'a Value) -> Result<&'a Array, Unwind> {
    match v {
        Value::Array(a) => Ok(a),
        other => Err(Unwind::type_error(format!(
            "{func}(): Argument #{} must be of type array, {} given",
            n + 1,
            rphp_runtime::value_name(&other)
        ))),
    }
}

/// Owned entries of an array argument.
fn entries_of(a: &Array) -> Vec<(ArrayKey, Value)> {
    a.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
}

/// Rebuild an array from entries, renumbering integer keys when `reindex`.
pub(crate) fn rebuild(entries: Vec<(ArrayKey, Value)>, reindex: bool) -> Array {
    let mut out = Array::new();
    for (k, v) in entries {
        match (reindex, &k) {
            (true, ArrayKey::Int(_)) => crate::arrays::push_any(&mut out, v),
            _ => crate::arrays::put(&mut out, k, v),
        }
    }
    out
}

/// A `TypeError` for a non-callable argument, in php's wording for the common
/// cases (a missing function name, or a non-string/array value).
fn callback_error(func: &str, pos: usize, name: Option<&str>, v: &Value, nullable: bool) -> Unwind {
    let what = match &*v.deref() {
        Value::Str(s) => format!("function \"{}\" not found or invalid function name", s.to_string_lossy()),
        Value::Array(a) if a.len() != 2 => "array callback must have exactly two members".to_string(),
        Value::Array(_) => "array callback must have exactly two members".to_string(),
        _ => "no array or string given".to_string(),
    };
    let nul = if nullable { " or null" } else { "" };
    match name {
        Some(n) => Unwind::type_error(format!("{func}(): Argument #{pos} (${n}) must be a valid callback{nul}, {what}")),
        None => Unwind::type_error(format!("{func}(): Argument #{pos} must be a valid callback{nul}, {what}")),
    }
}

/// Whether `v` can be invoked (the engine's shared callable resolution).
pub(crate) fn is_callable(ctx: &Ctx, v: &Value) -> bool {
    ctx.is_callable(v)
}

// ---- keys ---------------------------------------------------------------------------

/// `array_change_key_case(array $array, int $case = CASE_LOWER): array`.
pub(crate) fn array_change_key_case(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let arr = want_array("array_change_key_case", &args[0])?;
    // Anything but CASE_LOWER (0) means upper.
    let upper = args.get(1).map_or(0, Value::to_int) != 0;
    let mut out = Array::new();
    for (k, v) in arr.iter() {
        let nk = match k {
            ArrayKey::Str(s) => {
                let b = if upper { s.to_ascii_uppercase() } else { s.to_ascii_lowercase() };
                ArrayKey::Str(b.into_boxed_slice())
            }
            other => other.clone(),
        };
        out.set(nk, v.clone());
    }
    Ok(Value::Array(out))
}

/// `key_exists(mixed $key, array $array): bool` — the alias of
/// `array_key_exists` (which lives in `arrays.rs`).
pub(crate) fn key_exists(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    crate::arrays::array_key_exists(ctx, args)
}

// ---- set operations by key / with callbacks ------------------------------------------

/// How two entries are matched by a set operation.
struct SetMatch<'a> {
    /// The function name (for diagnostics).
    func: &'a str,
    /// Compare keys: `None` means "look the key up by hash".
    key_cb: Option<&'a Value>,
    /// Whether keys take part at all.
    by_key: bool,
    /// Compare values: `Some(cb)` a user callback, `None` internal string
    /// comparison; only when `by_value`.
    value_cb: Option<&'a Value>,
    by_value: bool,
}

fn key_equal(ctx: &mut Ctx, m: &SetMatch, a: &ArrayKey, b: &ArrayKey) -> Result<bool, Unwind> {
    match m.key_cb {
        Some(cb) => Ok(user_compare(ctx, m.func, cb, &a.to_value(), &b.to_value())? == Ordering::Equal),
        None => Ok(a == b),
    }
}

fn value_equal(ctx: &mut Ctx, m: &SetMatch, a: &Value, b: &Value) -> Result<bool, Unwind> {
    match m.value_cb {
        Some(cb) => Ok(user_compare(ctx, m.func, cb, a, b)? == Ordering::Equal),
        None => Ok(sort_string_of(ctx, a)? == sort_string_of(ctx, b)?),
    }
}

/// Whether `other` holds an entry matching `(k, v)` under `m`.
fn has_match(ctx: &mut Ctx, m: &SetMatch, other: &Array, k: &ArrayKey, v: &Value) -> Result<bool, Unwind> {
    if m.by_key && m.key_cb.is_none() {
        // Hash lookup on the key, then the value check if any.
        return match other.get(k) {
            None => Ok(false),
            Some(ov) => {
                if m.by_value {
                    value_equal(ctx, m, v, ov)
                } else {
                    Ok(true)
                }
            }
        };
    }
    for (ok, ov) in other.iter() {
        if m.by_key && !key_equal(ctx, m, k, ok)? {
            continue;
        }
        if m.by_value && !value_equal(ctx, m, v, ov)? {
            continue;
        }
        return Ok(true);
    }
    Ok(false)
}

/// The shared driver: `diff` keeps entries of the first array matched by no
/// other array, `intersect` those matched by every other array. `ncb` is the
/// number of trailing callback arguments.
fn set_op(ctx: &mut Ctx, func: &str, args: &[Value], diff: bool, by_key: bool, by_value: bool, ncb: usize) -> NativeResult {
    if args.len() < ncb + 1 {
        return Err(Unwind::argument_count_error(format!(
            "{func}() expects at least {} arguments, {} given",
            ncb + 1,
            args.len()
        )));
    }
    let narr = args.len() - ncb;
    for (i, a) in args[..narr].iter().enumerate() {
        if !matches!(a, Value::Array(_)) {
            return Err(if i == 0 {
                Unwind::type_error(format!(
                    "{func}(): Argument #1 ($array) must be of type array, {} given",
                    rphp_runtime::value_name(&a)
                ))
            } else {
                Unwind::type_error(format!(
                    "{func}(): Argument #{} must be of type array, {} given",
                    i + 1,
                    rphp_runtime::value_name(&a)
                ))
            });
        }
    }
    for (i, cb) in args[narr..].iter().enumerate() {
        if !is_callable(ctx, cb) {
            return Err(callback_error(func, narr + i + 1, None, cb, false));
        }
    }
    // With two callbacks the value one comes first, then the key one.
    let (value_cb, key_cb) = match ncb {
        0 => (None, None),
        1 if by_value && by_key => (None, Some(&args[narr])),
        1 if by_value => (Some(&args[narr]), None),
        1 => (None, Some(&args[narr])),
        _ => (Some(&args[narr]), Some(&args[narr + 1])),
    };
    begin_user_sort();
    let m = SetMatch { func, key_cb, by_key, value_cb, by_value };
    let base = want_array(func, &args[0])?;
    let others: Vec<&Array> = args[1..narr].iter().map(|a| want_array(func, a)).collect::<Result<_, _>>()?;
    let mut out = Array::new();
    for (k, v) in base.iter() {
        let mut keep = true;
        for other in &others {
            let hit = has_match(ctx, &m, other, k, v)?;
            if diff == hit {
                keep = false;
                break;
            }
        }
        if keep {
            crate::arrays::put(&mut out, k.clone(), v.clone());
        }
    }
    Ok(Value::Array(out))
}

/// `array_diff_key(array $array, array ...$arrays): array`.
pub(crate) fn array_diff_key(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    set_op(ctx, "array_diff_key", args, true, true, false, 0)
}

/// `array_diff_assoc(array $array, array ...$arrays): array`.
pub(crate) fn array_diff_assoc(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    set_op(ctx, "array_diff_assoc", args, true, true, true, 0)
}

/// `array_diff_ukey(array $array, array ...$arrays, callable $key_compare_func): array`.
pub(crate) fn array_diff_ukey(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    set_op(ctx, "array_diff_ukey", args, true, true, false, 1)
}

/// `array_diff_uassoc(array $array, array ...$arrays, callable $key_compare_func): array`.
pub(crate) fn array_diff_uassoc(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    set_op(ctx, "array_diff_uassoc", args, true, true, true, 1)
}

/// `array_udiff(array $array, array ...$arrays, callable $value_compare_func): array`.
pub(crate) fn array_udiff(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    set_op(ctx, "array_udiff", args, true, false, true, 1)
}

/// `array_udiff_assoc(array $array, array ...$arrays, callable $value_compare_func): array`.
pub(crate) fn array_udiff_assoc(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let n = args.len();
    if n < 2 {
        return Err(Unwind::argument_count_error(format!(
            "array_udiff_assoc() expects at least 2 arguments, {n} given"
        )));
    }
    // Keys by hash, values by the callback.
    let cb = args[n - 1].clone();
    if !is_callable(ctx, &cb) {
        return Err(callback_error("array_udiff_assoc", n, None, &cb, false));
    }
    begin_user_sort();
    let m = SetMatch { func: "array_udiff_assoc", key_cb: None, by_key: true, value_cb: Some(&cb), by_value: true };
    keyed_op(ctx, "array_udiff_assoc", &args[..n - 1], true, &m)
}

/// `array_udiff_uassoc(array $array, array ...$arrays, callable $value_compare_func, callable $key_compare_func): array`.
pub(crate) fn array_udiff_uassoc(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    set_op(ctx, "array_udiff_uassoc", args, true, true, true, 2)
}

/// `array_intersect_key(array $array, array ...$arrays): array`.
pub(crate) fn array_intersect_key(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    set_op(ctx, "array_intersect_key", args, false, true, false, 0)
}

/// `array_intersect_assoc(array $array, array ...$arrays): array`.
pub(crate) fn array_intersect_assoc(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    set_op(ctx, "array_intersect_assoc", args, false, true, true, 0)
}

/// `array_intersect_ukey(array $array, array ...$arrays, callable $key_compare_func): array`.
pub(crate) fn array_intersect_ukey(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    set_op(ctx, "array_intersect_ukey", args, false, true, false, 1)
}

/// `array_intersect_uassoc(array $array, array ...$arrays, callable $key_compare_func): array`.
pub(crate) fn array_intersect_uassoc(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    set_op(ctx, "array_intersect_uassoc", args, false, true, true, 1)
}

/// `array_uintersect(array $array, array ...$arrays, callable $value_compare_func): array`.
pub(crate) fn array_uintersect(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    set_op(ctx, "array_uintersect", args, false, false, true, 1)
}

/// `array_uintersect_assoc(array $array, array ...$arrays, callable $value_compare_func): array`.
pub(crate) fn array_uintersect_assoc(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let n = args.len();
    if n < 2 {
        return Err(Unwind::argument_count_error(format!(
            "array_uintersect_assoc() expects at least 2 arguments, {n} given"
        )));
    }
    let cb = args[n - 1].clone();
    if !is_callable(ctx, &cb) {
        return Err(callback_error("array_uintersect_assoc", n, None, &cb, false));
    }
    begin_user_sort();
    let m = SetMatch { func: "array_uintersect_assoc", key_cb: None, by_key: true, value_cb: Some(&cb), by_value: true };
    keyed_op(ctx, "array_uintersect_assoc", &args[..n - 1], false, &m)
}

/// `array_uintersect_uassoc(array $array, array ...$arrays, callable $value_compare_func, callable $key_compare_func): array`.
pub(crate) fn array_uintersect_uassoc(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    set_op(ctx, "array_uintersect_uassoc", args, false, true, true, 2)
}

/// `set_op` for a prepared matcher over `arrays` (all must be arrays).
fn keyed_op(ctx: &mut Ctx, func: &str, arrays: &[Value], diff: bool, m: &SetMatch) -> NativeResult {
    for (i, a) in arrays.iter().enumerate() {
        if i == 0 {
            want_array(func, a)?;
        } else {
            want_array_n(func, i, a)?;
        }
    }
    let base = want_array(func, &arrays[0])?;
    let others: Vec<&Array> = arrays[1..].iter().map(|a| want_array(func, a)).collect::<Result<_, _>>()?;
    let mut out = Array::new();
    for (k, v) in base.iter() {
        let mut keep = true;
        for other in &others {
            let hit = has_match(ctx, m, other, k, v)?;
            if diff == hit {
                keep = false;
                break;
            }
        }
        if keep {
            crate::arrays::put(&mut out, k.clone(), v.clone());
        }
    }
    Ok(Value::Array(out))
}

// ---- recursive merge / replace ---------------------------------------------------------

/// Merge `src` into `dst` the `array_merge_recursive` way: integer keys are
/// appended, string keys recurse (scalars on both sides become a list).
fn merge_recursive_into(dst: &mut Array, src: &Array) {
    for (k, v) in src.iter() {
        match k {
            ArrayKey::Int(_) => dst.push(v.clone()),
            ArrayKey::Str(_) => {
                let v = v.deref().into_owned();
                match dst.get_deref(k) {
                    None => dst.set(k.clone(), v),
                    Some(existing) => {
                        let mut merged = match existing {
                            Value::Array(a) => a,
                            scalar => {
                                let mut a = Array::new();
                                a.push(scalar);
                                a
                            }
                        };
                        match v {
                            Value::Array(sub) => merge_recursive_into(&mut merged, &sub),
                            scalar => merged.push(scalar),
                        }
                        dst.set(k.clone(), Value::Array(merged));
                    }
                }
            }
        }
    }
}

/// `array_merge_recursive(array ...$arrays): array`.
pub(crate) fn array_merge_recursive(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let mut out = Array::new();
    for (i, a) in args.iter().enumerate() {
        let arr = if i == 0 { want_array("array_merge_recursive", a)? } else { want_array_n("array_merge_recursive", i, a)? };
        merge_recursive_into(&mut out, arr);
    }
    Ok(Value::Array(out))
}

fn replace_recursive_into(dst: &mut Array, src: &Array) {
    for (k, v) in src.iter() {
        let v = v.deref().into_owned();
        match (dst.get_deref(k), v) {
            (Some(Value::Array(mut existing)), Value::Array(sub)) => {
                replace_recursive_into(&mut existing, &sub);
                dst.set(k.clone(), Value::Array(existing));
            }
            (_, v) => dst.set(k.clone(), v),
        }
    }
}

/// `array_replace_recursive(array $array, array ...$replacements): array`.
pub(crate) fn array_replace_recursive(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let mut out = want_array("array_replace_recursive", &args[0])?.clone();
    for (i, a) in args.iter().enumerate().skip(1) {
        replace_recursive_into(&mut out, want_array_n("array_replace_recursive", i, a)?);
    }
    Ok(Value::Array(out))
}

// ---- array_first / array_last (PHP 8.5) ----------------------------------------------------

/// `array_first(array $array): mixed` — the first value, or null when empty.
pub(crate) fn array_first(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let arr = want_array("array_first", &args[0])?;
    Ok(arr.first().map_or(Value::Null, |(_, v)| v.deref().into_owned()))
}

/// `array_last(array $array): mixed` — the last value, or null when empty.
pub(crate) fn array_last(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let arr = want_array("array_last", &args[0])?;
    Ok(arr.last().map_or(Value::Null, |(_, v)| v.deref().into_owned()))
}

// ---- array_find / any / all (PHP 8.4) ----------------------------------------------------

/// Shared body: the first entry whose `callback($value, $key)` is truthy.
fn find_entry(ctx: &mut Ctx, func: &str, args: &[Value]) -> Result<Option<(ArrayKey, Value)>, Unwind> {
    let arr = want_array(func, &args[0])?;
    if !is_callable(ctx, &args[1]) {
        return Err(callback_error(func, 2, Some("callback"), &args[1], false));
    }
    for (k, v) in entries_of(arr) {
        if ctx.call_value(&args[1], &[v.clone(), k.to_value()])?.to_bool() {
            return Ok(Some((k, v)));
        }
    }
    Ok(None)
}

/// `array_find(array $array, callable $callback): mixed`.
pub(crate) fn array_find(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    Ok(find_entry(ctx, "array_find", args)?.map_or(Value::Null, |(_, v)| v))
}

/// `array_find_key(array $array, callable $callback): mixed`.
pub(crate) fn array_find_key(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    Ok(find_entry(ctx, "array_find_key", args)?.map_or(Value::Null, |(k, _)| k.to_value()))
}

/// `array_any(array $array, callable $callback): bool`.
pub(crate) fn array_any(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    Ok(Value::Bool(find_entry(ctx, "array_any", args)?.is_some()))
}

/// `array_all(array $array, callable $callback): bool`.
pub(crate) fn array_all(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let arr = want_array("array_all", &args[0])?;
    if !is_callable(ctx, &args[1]) {
        return Err(callback_error("array_all", 2, Some("callback"), &args[1], false));
    }
    for (k, v) in entries_of(arr) {
        if !ctx.call_value(&args[1], &[v, k.to_value()])?.to_bool() {
            return Ok(Value::Bool(false));
        }
    }
    Ok(Value::Bool(true))
}

// ---- internal pointer ---------------------------------------------------------------------

/// `current(array|object $array): mixed`.
pub(crate) fn current(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let arr = want_array("current", &args[0])?;
    Ok(arr.pos_current().map_or(Value::Bool(false), |(_, v)| v.deref().into_owned()))
}

/// `key(array|object $array): int|string|null`.
pub(crate) fn key(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let arr = want_array("key", &args[0])?;
    Ok(arr.pos_current().map_or(Value::Null, |(k, _)| k.to_value()))
}

/// Shared body of the pointer movers: mutate the array in place and write it
/// back, returning the element under the new position (or `false`).
fn move_pointer(func: &str, args: &mut [Value], f: impl FnOnce(&mut Array) -> Option<Value>) -> NativeResult {
    let mut arr = want_array(func, &args[0])?.clone();
    let r = f(&mut arr).unwrap_or(Value::Bool(false));
    args[0] = Value::Array(arr);
    Ok(r)
}

/// `next(array|object &$array): mixed`.
pub(crate) fn next(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    move_pointer("next", args, |a| a.pos_advance().map(|(_, v)| v.deref().into_owned()))
}

/// `prev(array|object &$array): mixed`.
pub(crate) fn prev(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    move_pointer("prev", args, |a| a.pos_prev().map(|(_, v)| v.deref().into_owned()))
}

/// `reset(array|object &$array): mixed`.
pub(crate) fn reset(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    move_pointer("reset", args, |a| a.pos_rewind().map(|(_, v)| v.deref().into_owned()))
}

/// `end(array|object &$array): mixed`.
pub(crate) fn end(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    move_pointer("end", args, |a| a.pos_end().map(|(_, v)| v.deref().into_owned()))
}

// ---- natural sorts -------------------------------------------------------------------------

fn nat_sort(ctx: &mut Ctx, func: &str, args: &mut [Value], ci: bool) -> NativeResult {
    let mut entries = entries_of(want_array(func, &args[0])?);
    let flags = SORT_NATURAL | if ci { SORT_FLAG_CASE } else { 0 };
    sort_entries(&mut entries, |a, b| compare_values(ctx, flags, &a.1, &b.1))?;
    args[0] = Value::Array(rebuild(entries, false));
    Ok(Value::Bool(true))
}

/// `natsort(array &$array): true`.
pub(crate) fn natsort(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    nat_sort(ctx, "natsort", args, false)
}

/// `natcasesort(array &$array): true`.
pub(crate) fn natcasesort(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    nat_sort(ctx, "natcasesort", args, true)
}

// ---- array_walk ------------------------------------------------------------------------------

/// Turn reference cells nobody else holds back into plain values.
fn unwrap_lone_refs(arr: &mut Array) {
    let keys: Vec<ArrayKey> = arr.keys().cloned().collect();
    for k in keys {
        let inner = match arr.get(&k) {
            Some(Value::Ref(r)) if r.strong_count() == 1 => r.get(),
            _ => continue,
        };
        if let Some(slot) = arr.get_mut(&k) {
            *slot = inner;
        }
    }
}

/// Walk `arr` calling `cb($value, $key[, $arg])`; elements are passed as
/// reference cells so a by-reference callback parameter writes through.
fn walk(ctx: &mut Ctx, arr: &mut Array, cb: &Value, extra: Option<&Value>, recursive: bool) -> Result<(), Unwind> {
    let keys: Vec<ArrayKey> = arr.keys().cloned().collect();
    for k in keys {
        if recursive {
            if let Some(Value::Array(_)) = arr.get_deref(&k) {
                let mut sub = match arr.get_deref(&k) {
                    Some(Value::Array(a)) => a,
                    _ => unreachable!(),
                };
                walk(ctx, &mut sub, cb, extra, true)?;
                arr.set(k, Value::Array(sub));
                continue;
            }
        }
        let cell = arr.get_ref(k.clone());
        let mut call_args = vec![Value::Ref(cell), k.to_value()];
        if let Some(e) = extra {
            call_args.push(e.clone());
        }
        ctx.call_value(cb, &call_args)?;
    }
    unwrap_lone_refs(arr);
    Ok(())
}

/// `array_walk_recursive(array|object &$array, callable $callback, mixed $arg = UNKNOWN): true`.
pub(crate) fn array_walk_recursive(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let func = "array_walk_recursive";
    let mut arr = want_array(func, &args[0])?.clone();
    let cb = args[1].clone();
    if !is_callable(ctx, &cb) {
        return Err(callback_error(func, 2, Some("callback"), &cb, false));
    }
    let extra = args.get(2).cloned();
    walk(ctx, &mut arr, &cb, extra.as_ref(), true)?;
    args[0] = Value::Array(arr);
    Ok(Value::Bool(true))
}


// ---- array_multisort -----------------------------------------------------------------------------

/// `array_multisort(array &$array, mixed ...$rest): true` — sort several
/// arrays at once by the first, then the second, ...; each array may be
/// followed by one order (`SORT_ASC`/`SORT_DESC`) and one sort-flag value.
pub(crate) fn array_multisort(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    const SORT_ASC: i64 = 4;
    const SORT_DESC: i64 = 3;
    // (array index in args, descending?, flags)
    let mut specs: Vec<(usize, bool, i64)> = Vec::new();
    let mut order_seen = false;
    let mut flags_seen = false;
    for (i, a) in args.iter().enumerate() {
        match &*a.deref() {
            Value::Array(_) => {
                specs.push((i, false, SORT_REGULAR));
                order_seen = false;
                flags_seen = false;
            }
            Value::Int(n) if !specs.is_empty() => {
                let n = *n;
                if n == SORT_ASC || n == SORT_DESC {
                    if order_seen {
                        return Err(Unwind::type_error(format!(
                            "array_multisort(): Argument #{} must be an array or a sort flag that has not already been specified",
                            i + 1
                        )));
                    }
                    order_seen = true;
                    specs.last_mut().unwrap().1 = n == SORT_DESC;
                } else if matches!(n & !SORT_FLAG_CASE, SORT_REGULAR | SORT_NUMERIC | SORT_STRING | SORT_LOCALE_STRING | SORT_NATURAL) {
                    if flags_seen {
                        return Err(Unwind::type_error(format!(
                            "array_multisort(): Argument #{} must be an array or a sort flag that has not already been specified",
                            i + 1
                        )));
                    }
                    flags_seen = true;
                    specs.last_mut().unwrap().2 = n;
                } else {
                    return Err(Unwind::type_error(format!(
                        "array_multisort(): Argument #{} must be an array or a sort flag",
                        i + 1
                    )));
                }
            }
            _ => {
                return Err(Unwind::type_error(if i == 0 {
                    "array_multisort(): Argument #1 ($array) must be an array or a sort flag that has not already been specified".to_string()
                } else {
                    format!("array_multisort(): Argument #{} must be an array or a sort flag", i + 1)
                }));
            }
        }
    }
    let arrays: Vec<Vec<(ArrayKey, Value)>> = specs
        .iter()
        .map(|&(i, _, _)| match &*args[i].deref() {
            Value::Array(a) => entries_of(a),
            _ => unreachable!(),
        })
        .collect();
    let n = arrays[0].len();
    if arrays.iter().any(|a| a.len() != n) {
        return Err(Unwind::value_error("Array sizes are inconsistent"));
    }
    if n == 0 {
        return Ok(Value::Bool(true));
    }
    // Sort the row indices.
    let mut rows: Vec<usize> = (0..n).collect();
    let mut err: Option<Unwind> = None;
    zend_sort(&mut rows, &mut |&x, &y| {
        if err.is_some() {
            return Ordering::Equal;
        }
        for (col, &(_, desc, flags)) in specs.iter().enumerate() {
            let o = match compare_values(ctx, flags, &arrays[col][x].1, &arrays[col][y].1) {
                Ok(o) => o,
                Err(e) => {
                    err = Some(e);
                    return Ordering::Equal;
                }
            };
            let o = if desc { o.reverse() } else { o };
            if o != Ordering::Equal {
                return o;
            }
        }
        x.cmp(&y)
    });
    if let Some(e) = err {
        return Err(e);
    }
    // Rebuild every array in row order: string keys kept, int keys renumbered.
    for (col, &(i, _, _)) in specs.iter().enumerate() {
        let ordered: Vec<(ArrayKey, Value)> = rows.iter().map(|&r| arrays[col][r].clone()).collect();
        args[i] = Value::Array(rebuild(ordered, true));
    }
    Ok(Value::Bool(true))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zend_sort_orders_like_a_sort() {
        for n in [0usize, 1, 2, 3, 4, 5, 6, 7, 16, 17, 40, 100, 1500] {
            let mut v: Vec<i64> = (0..n as i64).map(|i| (i * 7919 + 13) % 101).collect();
            let mut expect = v.clone();
            expect.sort();
            zend_sort(&mut v, &mut |a, b| a.cmp(b));
            assert_eq!(v, expect, "n={n}");
        }
    }
}
