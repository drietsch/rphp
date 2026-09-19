//! `DateInterval` (php-src `ext/date/php_date.c`, the `date_interval_*`
//! half).
//!
//! **Why the fields are dynamic properties.** php shows ten keys for a
//! parsed duration and two — `from_string` and `date_string` — for one made
//! by `createFromDateString`, because both sets are synthesized from the C
//! struct on demand. A declared layout has one fixed shape, so the fields
//! are written as ordinary dynamic properties by whichever entry point
//! built the instance. They are the real state: `$i->d = 5` lands on the
//! property and `format('%d')` reads it back, exactly as php's read/write
//! handlers land on the struct.
//!
//! **`createFromDateString` keeps the string, not the fields.** php stores
//! the parsed relative time and shows only the string; the string is
//! re-scanned here whenever the interval is applied, which is what makes
//! `$d->add(DateInterval::createFromDateString('next monday'))` walk to a
//! weekday rather than add a fixed number of days.
//!
//! **The struct behind the string.** php keeps the parsed relative time of
//! a `createFromDateString` interval in the same C struct and only *shows*
//! the two keys, so `->d` is `2` for `'2 days'` while `var_dump` prints the
//! string. Here the two keys are the stored state and the fields are
//! derived from them by re-scanning ([`fields`]), reached through `__get` /
//! `__isset`.
//!
//! **Known divergences.**
//!
//! * `format('%a')` answers `(unknown)` for every interval that did not
//!   come out of `diff()`, as php does, but a `DateInterval` whose `days`
//!   property a caller set by hand is believed.
//! * `$i->from_string` and `$i->date_string` are readable here, and
//!   `isset()` answers true for them. php's read handler hides both — a
//!   warning and `null`, `isset()` false — while still showing them in
//!   `var_dump`, `(array)` and `serialize()`. One property bag serves both
//!   projections here, so it cannot answer differently to the two.
//! * Writing a field of a `createFromDateString` interval (`$i->d = 9`)
//!   lands in a *dynamic property*, which is what php deprecates for a name
//!   it does not know; php writes the hidden struct instead. Every later
//!   read agrees with php — the property wins over the re-scan for `->d`,
//!   `format()` and `add()` alike — but `var_dump` shows the extra key and
//!   the deprecation is php's own text for the wrong reason.

use rphp_runtime::{nm, Ctx, NativeResult, Registry, Unwind};
use rphp_value::{Array, ArrayKey, Object, Value};

use super::super::parse::{self, Parsed};
use super::dt::Diff;
use super::zone::snm;
use super::{date_throw, str_arg, this};

/// The ten keys php shows for a parsed duration, in php's order.
const DURATION_KEYS: [&[u8]; 10] = [
    b"y",
    b"m",
    b"d",
    b"h",
    b"i",
    b"s",
    b"f",
    b"invert",
    b"days",
    b"from_string",
];

/// Write the ten fields of a parsed duration onto an instance.
fn seed_duration(o: &Object, v: [i64; 6], f: f64, invert: i64, days: Value) {
    for (key, n) in [b"y".as_slice(), b"m", b"d", b"h", b"i", b"s"]
        .into_iter()
        .zip(v)
    {
        o.set(key, Value::Int(n));
    }
    o.set(b"f", Value::Float(f));
    o.set(b"invert", Value::Int(invert));
    o.set(b"days", days);
    o.set(b"from_string", Value::Bool(false));
}

/// Write the two fields of a `createFromDateString` interval.
fn seed_from_string(o: &Object, text: &[u8]) {
    o.set(b"from_string", Value::Bool(true));
    o.set(b"date_string", Value::string(text));
}

/// An integer field of an interval.
fn field(o: &Object, name: &[u8]) -> i64 {
    o.get_deref(name).map_or(0, |v| v.to_int())
}

/// The `f` field as whole microseconds, the way `%F` and `add()` want it.
fn micros(o: &Object) -> i64 {
    let f = o.get_deref(b"f").map_or(0.0, |v| v.to_float());
    (f * 1_000_000.0).round() as i64
}

/// The nine fields php answers for an interval, whichever entry point built
/// it: the six amounts, the fraction as whole microseconds, `invert`, and
/// `days` (`None` is php's `false`).
#[derive(Clone, Copy)]
struct Fields {
    v: [i64; 6],
    us: i64,
    invert: i64,
    days: Option<i64>,
}

/// The six amount keys, in php's order.
const AMOUNTS: [&[u8]; 6] = [b"y", b"m", b"d", b"h", b"i", b"s"];

/// Read the nine fields off an instance.
///
/// A `createFromDateString` interval shows only the string, but php keeps
/// the whole struct behind it, so the fields here are **derived**: the
/// string is re-scanned and its relative amounts are the answer. That is
/// why `->d` is `2` for `'2 days'`, `-2` for `'2 days ago'` (the sign is in
/// the amount, never in `invert`) and `0` for `'next monday'` — a weekday
/// walk is no fixed number of days. A property that is really there always
/// wins, which is what makes a written field stick.
fn fields(o: &Object) -> Fields {
    let mut f = Fields {
        v: [0; 6],
        us: 0,
        invert: 0,
        days: None,
    };
    if is_from_string(o) {
        let text = o
            .get_deref(b"date_string")
            .map(|v| v.to_php_bytes())
            .unwrap_or_default();
        let p = parse::parse(&text);
        f.v = [p.rel.y, p.rel.m, p.rel.d, p.rel.h, p.rel.i, p.rel.s];
        f.us = p.rel.us;
    }
    for (slot, key) in f.v.iter_mut().zip(AMOUNTS) {
        if let Some(v) = o.get_deref(key) {
            *slot = v.to_int();
        }
    }
    if let Some(v) = o.get_deref(b"f") {
        f.us = (v.to_float() * 1_000_000.0).round() as i64;
    }
    if let Some(v) = o.get_deref(b"invert") {
        f.invert = v.to_int();
    }
    match o.get_deref(b"days") {
        Some(Value::Bool(false)) | None => {}
        Some(v) => f.days = Some(v.to_int()),
    }
    f
}

/// The value php answers for a field name, or `None` when the name is not
/// one of the nine. Only a `createFromDateString` interval ever reaches
/// this: a parsed duration carries the nine as real properties, so the
/// magic accessors are never consulted for them.
fn readable(o: &Object, name: &[u8]) -> Option<Value> {
    if !is_from_string(o) {
        return None;
    }
    let f = fields(o);
    if let Some(i) = AMOUNTS.iter().position(|k| *k == name) {
        return Some(Value::Int(f.v[i]));
    }
    Some(match name {
        b"f" => Value::Float(f.us as f64 / 1_000_000.0),
        b"invert" => Value::Int(f.invert),
        b"days" => f.days.map_or(Value::Bool(false), Value::Int),
        _ => return None,
    })
}

/// Whether the interval is one `createFromDateString` built.
pub(crate) fn is_from_string(o: &Object) -> bool {
    o.get_deref(b"from_string").is_some_and(|v| v.to_bool())
}

// ---- the ISO 8601 duration grammar ---------------------------------------------

/// php's duration grammar: `PnYnMnWnDTnHnMnS`, plus the two combined
/// `P<date>T<time>` spellings. Fractions, signs and an empty designator
/// list are all rejected, which is why `PT1.5S` and a bare `P` are errors.
pub(crate) fn parse_duration(s: &[u8]) -> Option<[i64; 6]> {
    let rest = s.strip_prefix(b"P")?;
    if let Some(v) = parse_combined(rest) {
        return Some(v);
    }
    let mut out = [0i64; 6];
    let mut weeks = 0i64;
    let mut i = 0;
    let mut in_time = false;
    let mut seen = false;
    while i < rest.len() {
        if rest[i] == b'T' {
            if in_time {
                return None;
            }
            in_time = true;
            i += 1;
            continue;
        }
        let start = i;
        while i < rest.len() && rest[i].is_ascii_digit() {
            i += 1;
        }
        if i == start || i == rest.len() {
            return None;
        }
        let n: i64 = std::str::from_utf8(&rest[start..i]).ok()?.parse().ok()?;
        let slot = match (in_time, rest[i]) {
            (false, b'Y') => 0,
            (false, b'M') => 1,
            (false, b'D') => 2,
            (false, b'W') => {
                weeks += n;
                i += 1;
                seen = true;
                continue;
            }
            (true, b'H') => 3,
            (true, b'M') => 4,
            (true, b'S') => 5,
            _ => return None,
        };
        out[slot] += n;
        i += 1;
        seen = true;
    }
    if !seen {
        return None;
    }
    out[2] += weeks * 7;
    Some(out)
}

/// The combined form `P0003-06-04T12:30:05`. php wants both halves, both
/// separators and exactly two digits in every field but the year, so
/// `P00030604T123005` and `P0003-06-04T12:30` are both errors.
fn parse_combined(rest: &[u8]) -> Option<[i64; 6]> {
    let t = rest.iter().position(|&c| c == b'T')?;
    let d = split_fixed(&rest[..t], b'-')?;
    let h = split_fixed(&rest[t + 1..], b':')?;
    Some([d[0], d[1], d[2], h[0], h[1], h[2]])
}

/// `a<sep>bb<sep>cc`, where only the first field may be longer than two
/// digits.
fn split_fixed(s: &[u8], sep: u8) -> Option<[i64; 3]> {
    let mut parts = s.split(|&c| c == sep);
    let mut out = [0i64; 3];
    for (i, slot) in out.iter_mut().enumerate() {
        let part = parts.next()?;
        if part.is_empty() || !part.iter().all(u8::is_ascii_digit) || (i > 0 && part.len() != 2) {
            return None;
        }
        *slot = std::str::from_utf8(part).ok()?.parse().ok()?;
    }
    if parts.next().is_some() {
        return None;
    }
    Some(out)
}

// ---- construction ------------------------------------------------------------------

/// `DateInterval::__construct(string $duration)`
fn construct(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let text = str_arg(ctx, "DateInterval::__construct", 1, "duration", &args[0])?;
    match parse_duration(&text) {
        Some(v) => {
            seed_duration(o, v, 0.0, 0, Value::Bool(false));
            Ok(Value::Null)
        }
        None => Err(date_throw(
            ctx,
            "DateMalformedIntervalStringException",
            format!("Unknown or bad format ({})", String::from_utf8_lossy(&text)),
        )),
    }
}

/// `DateInterval::createFromDateString(string $datetime): DateInterval`
///
/// The string must be **purely relative**: php runs its ordinary
/// `strtotime` scanner and then refuses anything that pinned an absolute
/// field down, so `'2 days'` and `'midnight'` are intervals while
/// `'10:00'`, `'2021-01-01'` and `'2 days UTC'` are not. `midnight` writes
/// the clock without *having* a time, which is the distinction php draws
/// and the one `Parsed`'s three `have_` flags carry.
pub(crate) fn create_from_date_string(
    ctx: &mut Ctx,
    _: Option<&Object>,
    args: &mut [Value],
) -> NativeResult {
    let who = "DateInterval::createFromDateString";
    let text = str_arg(ctx, who, 1, "datetime", &args[0])?;
    let p = parse::parse(&text);
    if let Some(err) = p.errors.first().copied() {
        return Err(bad_interval_format(ctx, &text, err));
    }
    if p.have_date || p.have_time > 0 || p.have_zone {
        return Err(date_throw(
            ctx,
            "DateMalformedIntervalStringException",
            format!(
                "String '{}' contains non-relative elements",
                String::from_utf8_lossy(&text)
            ),
        ));
    }
    let o = new_interval(ctx)?;
    seed_from_string(&o, &text);
    Ok(Value::Object(o))
}

/// php's `Unknown or bad format (…) at position n (c): …` — the interval
/// half of the date extension reports a parse failure in its own words,
/// not in `DateTime`'s `Failed to parse time string`.
pub(crate) fn bad_interval_format(
    ctx: &mut Ctx,
    text: &[u8],
    err: (usize, &'static str),
) -> Unwind {
    let (pos, msg) = err;
    let ch = text.get(pos).copied().unwrap_or(b' ') as char;
    date_throw(
        ctx,
        "DateMalformedIntervalStringException",
        format!(
            "Unknown or bad format ({}) at position {pos} ({ch}): {msg}",
            String::from_utf8_lossy(text)
        ),
    )
}

/// A bare `DateInterval` instance with no fields yet.
fn new_interval(ctx: &mut Ctx) -> Result<Object, Unwind> {
    let cid = ctx
        .class_by_name(b"DateInterval")
        .ok_or_else(|| Unwind::error("Class \"DateInterval\" not found"))?;
    Ok(ctx.instantiate(cid))
}

/// The `DateInterval` a `diff()` answers: the same ten keys, with `days`
/// filled in.
pub(crate) fn from_diff(ctx: &mut Ctx, r: &Diff) -> NativeResult {
    let o = new_interval(ctx)?;
    seed_duration(
        &o,
        [r.y, r.m, r.d, r.h, r.i, r.s],
        r.us as f64 / 1_000_000.0,
        r.invert,
        Value::Int(r.days),
    );
    Ok(Value::Object(o))
}

/// The relative amounts an interval contributes to `add()` / `sub()`. A
/// `createFromDateString` interval is re-scanned, so its weekday walks and
/// `first day of` survive; a parsed duration becomes a plain field bag.
pub(crate) fn relative_of(ctx: &mut Ctx, o: &Object) -> Result<Parsed, Unwind> {
    if is_from_string(o) {
        let text = o
            .get_deref(b"date_string")
            .map(|v| v.to_php_bytes())
            .unwrap_or_default();
        let mut p = parse::parse(&text);
        if let Some(err) = p.errors.first().copied() {
            return Err(bad_interval_format(ctx, &text, err));
        }
        // The scan carries what no field can — the weekday walk, `first day
        // of` — and the amounts come back through `fields`, so a field the
        // caller wrote by hand is the one that applies.
        let f = fields(o);
        [p.rel.y, p.rel.m, p.rel.d, p.rel.h, p.rel.i, p.rel.s] = f.v;
        p.rel.us = f.us;
        return Ok(p);
    }
    let sign = if field(o, b"invert") != 0 { -1 } else { 1 };
    let mut p = Parsed {
        have_relative: true,
        ..Parsed::default()
    };
    p.rel.y = sign * field(o, b"y");
    p.rel.m = sign * field(o, b"m");
    p.rel.d = sign * field(o, b"d");
    p.rel.h = sign * field(o, b"h");
    p.rel.i = sign * field(o, b"i");
    p.rel.s = sign * field(o, b"s");
    p.rel.us = sign * micros(o);
    Ok(p)
}

// ---- format -----------------------------------------------------------------------

/// `DateInterval::format(string $format): string`
pub(crate) fn format(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let fmt = str_arg(ctx, "DateInterval::format", 1, "format", &args[0])?;
    Ok(Value::string(&render(o, &fmt)))
}

/// php's `date_interval_format`. An unknown specifier is written back
/// verbatim (`%Q` stays `%Q`) and a trailing `%` disappears.
fn render(o: &Object, fmt: &[u8]) -> Vec<u8> {
    let fs = fields(o);
    let mut out = Vec::with_capacity(fmt.len());
    let mut i = 0;
    while i < fmt.len() {
        if fmt[i] != b'%' {
            out.push(fmt[i]);
            i += 1;
            continue;
        }
        i += 1;
        let Some(&c) = fmt.get(i) else { break };
        i += 1;
        let piece = match c {
            b'Y' => format!("{:02}", fs.v[0]),
            b'y' => fs.v[0].to_string(),
            b'M' => format!("{:02}", fs.v[1]),
            b'm' => fs.v[1].to_string(),
            b'D' => format!("{:02}", fs.v[2]),
            b'd' => fs.v[2].to_string(),
            b'H' => format!("{:02}", fs.v[3]),
            b'h' => fs.v[3].to_string(),
            b'I' => format!("{:02}", fs.v[4]),
            b'i' => fs.v[4].to_string(),
            b'S' => format!("{:02}", fs.v[5]),
            b's' => fs.v[5].to_string(),
            b'F' => format!("{:06}", fs.us),
            b'f' => fs.us.to_string(),
            b'R' => (if fs.invert != 0 { "-" } else { "+" }).to_string(),
            b'r' => (if fs.invert != 0 { "-" } else { "" }).to_string(),
            b'a' => match fs.days {
                None => "(unknown)".to_string(),
                Some(n) => n.to_string(),
            },
            b'%' => "%".to_string(),
            other => {
                out.push(b'%');
                out.push(other);
                continue;
            }
        };
        out.extend_from_slice(piece.as_bytes());
    }
    out
}

// ---- the magic accessors ----------------------------------------------------------

/// `DateInterval::__get(string $name): mixed`.
///
/// php answers a `createFromDateString` interval's nine struct fields
/// through its read handler while showing only the string, so the read is
/// routed here — see [`fields`]. Every other name is php's plain
/// undefined-property read: a warning and `null`.
fn magic_get(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let name = args[0].deref().to_php_bytes();
    if let Some(v) = readable(o, &name) {
        return Ok(v);
    }
    let class = String::from_utf8_lossy(o.layout().class_name()).into_owned();
    ctx.warn(&format!(
        "Undefined property: {class}::${}",
        String::from_utf8_lossy(&name)
    ))?;
    Ok(Value::Null)
}

/// `DateInterval::__isset(string $name): bool` — the nine derived fields
/// are set, nothing else is.
fn magic_isset(_: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let name = args[0].deref().to_php_bytes();
    Ok(Value::Bool(readable(o, &name).is_some()))
}

// ---- serialization ----------------------------------------------------------------

/// The property bag php serializes: the two `createFromDateString` keys, or
/// the ten of a parsed duration.
fn state_array(o: &Object) -> Array {
    let mut a = Array::new();
    if is_from_string(o) {
        a.set(ArrayKey::str(b"from_string"), Value::Bool(true));
        a.set(
            ArrayKey::str(b"date_string"),
            o.get_deref(b"date_string")
                .unwrap_or_else(|| Value::string(b"")),
        );
        return a;
    }
    for key in DURATION_KEYS {
        if let Some(v) = o.get_deref(key) {
            a.set(ArrayKey::str(key), v);
        }
    }
    a
}

/// `DateInterval::__serialize(): array`
fn magic_serialize(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    Ok(Value::Array(state_array(this(o)?)))
}

/// Copy a property bag onto an instance, keeping php's key order.
fn restore(o: &Object, bag: &Array) {
    if bag
        .get_deref(&ArrayKey::str(b"from_string"))
        .is_some_and(|v| v.to_bool())
    {
        o.set(b"from_string", Value::Bool(true));
        o.set(
            b"date_string",
            bag.get_deref(&ArrayKey::str(b"date_string"))
                .unwrap_or_else(|| Value::string(b"")),
        );
        return;
    }
    for key in DURATION_KEYS {
        let fallback = match key {
            b"f" => Value::Float(0.0),
            b"days" | b"from_string" => Value::Bool(false),
            _ => Value::Int(0),
        };
        o.set(key, bag.get_deref(&ArrayKey::str(key)).unwrap_or(fallback));
    }
}

/// The array argument of `__unserialize` / `__set_state`.
fn bag_arg(who: &str, v: &Value) -> Result<Array, Unwind> {
    match &*v.deref() {
        Value::Array(a) => Ok(a.clone()),
        _ => Err(Unwind::type_error(format!(
            "{who}(): Argument #1 ($data) must be of type array"
        ))),
    }
}

/// `DateInterval::__unserialize(array $data): void`
fn magic_unserialize(_: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    let bag = bag_arg("DateInterval::__unserialize", &args[0])?;
    restore(o, &bag);
    Ok(Value::Null)
}

/// `DateInterval::__wakeup(): void` — the properties are the state, so the
/// pre-8.2 form has nothing left to do.
fn magic_wakeup(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    this(o)?;
    Ok(Value::Null)
}

/// `DateInterval::__set_state(array $array): DateInterval`
fn set_state(ctx: &mut Ctx, _: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let bag = bag_arg("DateInterval::__set_state", &args[0])?;
    let o = new_interval(ctx)?;
    restore(&o, &bag);
    Ok(Value::Object(o))
}

/// Register `DateInterval`.
pub(crate) fn register_classes(r: &mut Registry) {
    r.class("DateInterval")
        .method("__construct", nm!(1, Some(1), construct))
        .method(
            "createFromDateString",
            snm(1, Some(1), create_from_date_string),
        )
        .method("format", nm!(1, Some(1), format))
        .method("__get", nm!(1, Some(1), magic_get))
        .method("__isset", nm!(1, Some(1), magic_isset))
        .method("__serialize", nm!(0, Some(0), magic_serialize))
        .method("__unserialize", nm!(1, Some(1), magic_unserialize))
        .method("__wakeup", nm!(0, Some(0), magic_wakeup))
        .method("__set_state", snm(1, Some(1), set_state))
        .finish();
}
