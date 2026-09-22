//! Stream filters (php-src `ext/standard/filters.c`): the chain a stream's
//! bytes pass through on their way in or out.
//!
//! A filter is attached to one stream, in one direction, and keeps its own
//! state — `dechunk` is the reason the state matters, since a chunk
//! boundary need not fall where a write does. Symfony's `NativeHttpClient`
//! attaches exactly this: `stream_filter_append($buffer, 'dechunk',
//! STREAM_FILTER_WRITE)` on a `php://temp`, then writes a response body
//! into it and reads the decoded bytes back out.
//!
//! **Where a filter runs.** A write filter transforms what `fwrite()` hands
//! over, before it reaches the buffer or the descriptor — and `fwrite()`
//! still answers the number of bytes it was *given*, not the number that
//! came out, as php does. A read filter transforms what the stream has not
//! yet handed out: for a descriptor that is each read as it happens, and
//! for a buffer — where the bytes are already there — the rest of the
//! buffer, transformed when the filter is attached.
//!
//! **Known divergence.** The filters here are the ones that need nothing
//! but this crate: `dechunk`, `string.rot13`, `string.toupper`,
//! `string.tolower` and `consumed`. php also registers `zlib.*`,
//! `bzip2.*`, `convert.iconv.*` and `convert.*`, and lets a script add its
//! own with `stream_filter_register()` and a `php_user_filter` subclass.
//! `stream_get_filters()` lists what is really here rather than php's
//! longer list, since the whole point of the call is a feature test.

use rphp_runtime::{nf, Ctx, NativeFn, NativeResult, Unwind};
use rphp_value::{Array, ArrayKey, Value};

/// This extension's registry contribution (see `lib.rs`).
pub(crate) static FUNCTIONS: &[NativeFn] = &[
    nf!("stream_filter_append", 2, Some(4), stream_filter_append),
    nf!("stream_filter_prepend", 2, Some(4), stream_filter_prepend),
    nf!("stream_filter_remove", 1, Some(1), stream_filter_remove),
    nf!("stream_get_filters", 0, Some(0), stream_get_filters),
];

/// The filters this build registers, in the order `stream_get_filters()`
/// lists them.
const FILTERS: &[&str] =
    &["string.rot13", "string.toupper", "string.tolower", "consumed", "dechunk"];

/// One attached filter: what it does, and what it has seen so far.
pub(crate) enum Filter {
    Dechunk(crate::dechunk::Dechunk),
    Rot13,
    ToUpper,
    ToLower,
    /// php's `consumed`: passes everything through and counts it.
    Consumed(u64),
}

impl Filter {
    /// Build one by the name php knows it as.
    fn named(name: &str) -> Option<Filter> {
        Some(match name {
            "dechunk" => Filter::Dechunk(crate::dechunk::Dechunk::default()),
            "string.rot13" => Filter::Rot13,
            "string.toupper" => Filter::ToUpper,
            "string.tolower" => Filter::ToLower,
            "consumed" => Filter::Consumed(0),
            _ => return None,
        })
    }

    /// Pass `input` through, appending what comes out to `out`.
    fn apply(&mut self, input: &[u8], out: &mut Vec<u8>) {
        match self {
            Filter::Dechunk(d) => d.push(input, out),
            Filter::Rot13 => out.extend(input.iter().map(|b| rot13(*b))),
            Filter::ToUpper => out.extend(input.iter().map(u8::to_ascii_uppercase)),
            Filter::ToLower => out.extend(input.iter().map(u8::to_ascii_lowercase)),
            Filter::Consumed(n) => {
                *n += input.len() as u64;
                out.extend_from_slice(input);
            }
        }
    }
}

fn rot13(b: u8) -> u8 {
    match b {
        b'a'..=b'z' => (b - b'a' + 13) % 26 + b'a',
        b'A'..=b'Z' => (b - b'A' + 13) % 26 + b'A',
        other => other,
    }
}

/// An attached filter, as the chain holds it.
pub(crate) struct Attached {
    /// The id of the `stream filter` resource that stands for it, so
    /// `stream_filter_remove()` can find it again.
    pub(crate) id: u32,
    pub(crate) filter: Filter,
}

/// Run a chain over some bytes.
pub(crate) fn run(chain: &mut [Attached], input: &[u8]) -> Vec<u8> {
    let mut data = input.to_vec();
    for a in chain.iter_mut() {
        let mut out = Vec::with_capacity(data.len());
        a.filter.apply(&data, &mut out);
        data = out;
    }
    data
}

/// What a `stream filter` resource carries: which stream it is on, so
/// removing it can find the chain again.
pub(crate) struct FilterHandle {
    pub(crate) stream: Value,
    pub(crate) read: bool,
}

/// `stream_filter_append(resource $stream, string $filtername, int $mode = 0, mixed $params = null): resource|false`
fn stream_filter_append(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    attach(ctx, args, "stream_filter_append", false)
}

/// `stream_filter_prepend(resource $stream, string $filtername, int $mode = 0, mixed $params = null): resource|false`
fn stream_filter_prepend(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    attach(ctx, args, "stream_filter_prepend", true)
}

fn attach(ctx: &mut Ctx, args: &mut [Value], func: &str, first: bool) -> NativeResult {
    const READ: i64 = 1;
    const WRITE: i64 = 2;
    let stream = args[0].clone();
    let name = String::from_utf8_lossy(&args[1].to_php_bytes()).into_owned();
    let mode = args.get(2).map_or(0, Value::to_int);
    let Some(filter) = Filter::named(&name) else {
        ctx.warn(&format!("{func}(): Unable to locate filter \"{name}\""))?;
        return Ok(Value::Bool(false));
    };
    // php's default mode is both directions; a stream opened one way only
    // uses the half that applies.
    let (want_read, want_write) = if mode & (READ | WRITE) == 0 {
        (true, true)
    } else {
        (mode & READ != 0, mode & WRITE != 0)
    };
    // One resource stands for the attachment, whichever end it is on.
    let read = want_read && !want_write;
    let handle = ctx.resources.add(
        "stream filter",
        Box::new(FilterHandle { stream: stream.clone(), read }),
    );
    let Value::Resource(r) = &handle else {
        return Ok(Value::Bool(false));
    };
    let id = r.id();
    let attached = Attached { id, filter };
    let ok = crate::file::attach_filter(ctx, &stream, func, attached, want_read, first)?;
    if !ok {
        ctx.resources.close_value(&handle);
        return Ok(Value::Bool(false));
    }
    Ok(handle)
}

/// `stream_filter_remove(resource $stream_filter): bool`
fn stream_filter_remove(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let Value::Resource(r) = &*args[0].deref() else {
        return Err(Unwind::type_error(format!(
            "stream_filter_remove(): Argument #1 ($stream_filter) must be of type resource, {} given",
            rphp_runtime::value_name(&args[0].deref())
        )));
    };
    if r.kind() != "stream filter" {
        ctx.warn("stream_filter_remove(): Invalid resource given, not a stream filter")?;
        return Ok(Value::Bool(false));
    }
    let id = r.id();
    let (stream, read) = {
        let mut payload = r.payload_mut();
        let Some(h) = payload.as_mut().and_then(|a| a.downcast_mut::<FilterHandle>()) else {
            return Ok(Value::Bool(false));
        };
        (h.stream.clone(), h.read)
    };
    let removed = crate::file::remove_filter(ctx, &stream, id, read)?;
    if removed {
        ctx.resources.close(id);
    }
    Ok(Value::Bool(removed))
}

/// `stream_get_filters(): array`
fn stream_get_filters(_ctx: &mut Ctx, _args: &mut [Value]) -> NativeResult {
    let mut out = Array::new();
    for (i, f) in FILTERS.iter().enumerate() {
        out.set(ArrayKey::Int(i as i64), Value::string(f.as_bytes()));
    }
    Ok(Value::Array(out))
}
