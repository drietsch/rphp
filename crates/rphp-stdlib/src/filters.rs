//! Stream filters (php-src `main/streams/filter.c`, `ext/standard/filters.c`
//! and `streamsfuncs.c`): the chains a stream's bytes pass through on their
//! way in or out, the factories that build them by name, and the
//! `php://filter` wrapper's filter list.
//!
//! A filter is attached to one stream, in one direction, and keeps its own
//! state — `dechunk` is the reason the state matters, since a chunk
//! boundary need not fall where a write does. Symfony's `NativeHttpClient`
//! attaches exactly this: `stream_filter_append($buffer, 'dechunk',
//! STREAM_FILTER_WRITE)` on a `php://temp`, then writes a response body
//! into it and reads the decoded bytes back out.
//!
//! **The chain.** Bytes travel as php's *buckets*: a write is one bucket, a
//! read pulls the underlying stream a chunk (8192 bytes) at a time. Each
//! filter takes the buckets the previous one passed on and answers
//! `PSFS_PASS_ON` (here are mine), `PSFS_FEED_ME` (nothing yet) or
//! `PSFS_ERR_FATAL`; the chain stops at the first filter that does not pass
//! on. Filters are one of three kinds: the byte-at-a-time `string.*` ones,
//! a stateful [`NativeFilter`] (`dechunk` aside, which predates the trait),
//! or a script's `php_user_filter` subclass (`user_filters.rs`), whose
//! `filter()` method is called with real bucket brigades.
//!
//! **Where a filter runs.** A write chain runs on `fwrite()` and friends,
//! and `fwrite()` answers what the *first* filter says it consumed — every
//! native filter consumes everything, so that is the length given; a user
//! filter that leaves `$consumed` alone makes it `0`, as in php. A read
//! chain runs as reads need bytes: what it produced waits in the stream's
//! [`FilterState`] view, which the read functions serve instead of the raw
//! buffer. The underlying stream running dry is the chain's `$closing` call.
//!
//! **Flush and close.** `fflush()` and a seek flush the write chain with
//! `PSFS_FLAG_FLUSH_INC`; `fclose()` — and the end of the request, for a
//! stream still open — with `PSFS_FLAG_FLUSH_CLOSE`, which is when a
//! filter emits what it held back (base64's last quantum, a compressor's
//! trailer). Removing a filter flushes it, and whatever follows it, first.
//! A native filter sees the same [`Flush`] through [`NativeFilter::filter`].
//!
//! **Adding a native filter.** Implement [`NativeFilter`] and add a factory
//! arm to [`builtin_create`] plus its name to [`FACTORIES`] (a `prefix.*`
//! name serves every `prefix.<anything>`). `zlib.*` is meant to slot in
//! exactly so, ahead of the rest of the list as php orders it.
//!
//! **Known divergences.** Resources here are never freed by refcount, so a
//! stream whose last variable goes away is closed (and its filters flushed)
//! only at the end of the request, where php closes it at once; and php's
//! read buffer on a `php://memory`/`php://temp` stream is not modelled, so a
//! read filter appended after part of such a stream was read sees the rest
//! in reads rather than as "pre-buffered data".

mod convert;

use rphp_runtime::{nf, Ctx, NativeFn, NativeResult, Unwind};
use rphp_value::{Array, ArrayKey, Value};

/// This extension's registry contribution (see `lib.rs`).
pub(crate) static FUNCTIONS: &[NativeFn] = &[
    nf!("stream_filter_append", 2, Some(4), stream_filter_append),
    nf!("stream_filter_prepend", 2, Some(4), stream_filter_prepend),
    nf!("stream_filter_remove", 1, Some(1), stream_filter_remove),
    nf!("stream_get_filters", 0, Some(0), stream_get_filters),
];

/// The factories this build registers, in the order `stream_get_filters()`
/// lists them (php's hash order; user filters follow).
const FACTORIES: &[&str] = &[
    "convert.iconv.*",
    "string.rot13",
    "string.toupper",
    "string.tolower",
    "convert.*",
    "consumed",
    "dechunk",
];

/// The chunk a read pulls from the underlying stream (php's `chunk_size`).
pub(crate) const CHUNK: usize = 8192;

/// `STREAM_FILTER_READ` / `STREAM_FILTER_WRITE`.
const READ: i64 = 1;
const WRITE: i64 = 2;

/// Which of php's `PSFS_FLAG_*` a filter call carries.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Flush {
    /// `PSFS_FLAG_NORMAL`: more is coming.
    Normal,
    /// `PSFS_FLAG_FLUSH_INC`: `fflush()` or a seek.
    Inc,
    /// `PSFS_FLAG_FLUSH_CLOSE`: the stream is closing, or the underlying
    /// stream ran dry — the last call, where held-back output comes out.
    Close,
}

/// A stateful native filter. `input` is everything the call's buckets
/// hold; what comes out is appended to `out`. An `Err` is php's
/// `PSFS_ERR_FATAL`, and says what php reports with it.
pub(crate) trait NativeFilter {
    fn filter(&mut self, input: &[u8], out: &mut Vec<u8>, flush: Flush) -> Result<(), Fault>;
}

/// How a native filter's failure is reported: the message goes out with
/// the calling function's prefix (`fwrite(): Stream filter (…): …`).
#[derive(Debug)]
// `Silent` and `Notice` are for the filters still to plug in (`zlib.*`).
#[allow(dead_code)]
pub(crate) enum Fault {
    Silent,
    Warning(String),
    /// zlib's `zlib: data error` is a notice in php.
    Notice(String),
}

/// php's `php_stream_filter_status_t`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Status {
    Fatal,
    FeedMe,
    PassOn,
    /// A user filter's answer that is none of the three: php's switch has
    /// no case for it, so nothing is written and nothing fails.
    Other,
}

/// One attached filter: what it does, and what it has seen so far.
pub(crate) enum Filter {
    Dechunk(crate::dechunk::Dechunk),
    Rot13,
    ToUpper,
    ToLower,
    /// php's `consumed`: passes everything through and counts it.
    Consumed(u64),
    Native(Box<dyn NativeFilter>),
    User(crate::user_filters::UserFilter),
}

impl Filter {
    /// One filter call: `input` in, the buckets it passes on out.
    fn run(
        &mut self,
        ctx: &mut Ctx,
        stream: &Value,
        mut input: Vec<Vec<u8>>,
        consumed: Option<&mut i64>,
        flush: Flush,
        func: &str,
    ) -> Result<(Status, Vec<Vec<u8>>), Unwind> {
        let total: usize = input.iter().map(Vec::len).sum();
        let map = |input: &mut Vec<Vec<u8>>, f: fn(u8) -> u8| {
            for b in input.iter_mut() {
                b.iter_mut().for_each(|c| *c = f(*c));
            }
        };
        let joined = |input: &[Vec<u8>]| input.concat();
        let out = match self {
            Filter::User(u) => return u.call(ctx, stream, input, consumed, flush == Flush::Close, func),
            Filter::Rot13 => {
                map(&mut input, rot13);
                input
            }
            Filter::ToUpper => {
                map(&mut input, |c| c.to_ascii_uppercase());
                input
            }
            Filter::ToLower => {
                map(&mut input, |c| c.to_ascii_lowercase());
                input
            }
            Filter::Consumed(n) => {
                *n += total as u64;
                input
            }
            Filter::Dechunk(d) => {
                let mut out = Vec::new();
                d.push(&joined(&input), &mut out);
                vec![out]
            }
            Filter::Native(f) => {
                let mut out = Vec::new();
                if let Err(fault) = f.filter(&joined(&input), &mut out, flush) {
                    match fault {
                        Fault::Silent => {}
                        Fault::Warning(w) => ctx.warn(&format!("{func}(): {w}"))?,
                        Fault::Notice(n) => ctx.notice(&format!("{func}(): {n}"))?,
                    }
                    return Ok((Status::Fatal, Vec::new()));
                }
                vec![out]
            }
        };
        if let Some(c) = consumed {
            *c += total as i64;
        }
        Ok((Status::PassOn, out.into_iter().filter(|b| !b.is_empty()).collect()))
    }

    /// php's `dtor`: a user filter's `onClose()`.
    fn dispose(&mut self, ctx: &mut Ctx) -> Result<(), Unwind> {
        match self {
            Filter::User(u) => u.on_close(ctx),
            _ => Ok(()),
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
    /// Which attachment this is, so `stream_filter_remove()` can find it.
    pub(crate) id: u32,
    /// The `stream filter` resource that stands for it, if one was handed
    /// out (php hands out one per call, for the write half of a both-ways
    /// attachment); it is closed with the stream.
    pub(crate) res: Option<u32>,
    pub(crate) filter: Filter,
}

/// A stream's filter bookkeeping beyond the chains themselves (`file.rs`
/// keeps one in every stream).
#[derive(Default)]
pub(crate) struct FilterState {
    /// What the read chain produced that reads have not handed out yet, and
    /// the read cursor into it: while a read chain is attached, the read
    /// functions serve this instead of the stream's own buffer.
    pub(crate) view: Vec<u8>,
    pub(crate) vpos: usize,
    pub(crate) veof: bool,
    /// The stream position of `view[0]`, for `ftell()`.
    pub(crate) vbase: usize,
    /// The underlying stream ran dry and the chain had its closing call.
    pub(crate) done: bool,
    /// A filter callback is running (php's `PHP_STREAM_FLAG_NO_FCLOSE`).
    pub(crate) busy: bool,
    /// The `php://filter/…` url this stream was opened under.
    pub(crate) url: Option<Box<str>>,
}

impl FilterState {
    /// Whether reads are served from the view.
    pub(crate) fn view_active(&self, has_read_filters: bool) -> bool {
        has_read_filters || self.vpos < self.view.len()
    }

    /// Throw the view away after a seek to `pos`.
    pub(crate) fn reset_view(&mut self, pos: usize) {
        self.view.clear();
        self.vpos = 0;
        self.vbase = pos;
        self.veof = false;
        self.done = false;
    }

    fn avail(&self) -> &[u8] {
        &self.view[self.vpos.min(self.view.len())..]
    }

    /// Append what the chain produced, dropping what was handed out.
    fn push_view(&mut self, bytes: &[u8]) {
        if self.vpos > 0 && self.vpos >= self.view.len() {
            self.vbase += self.vpos;
            self.view.clear();
            self.vpos = 0;
        }
        self.view.extend_from_slice(bytes);
    }
}

// ---- factories -----------------------------------------------------------------------

/// Per-request filter bookkeeping: the serial attachments are numbered by.
#[derive(Default)]
struct Serial(u32);

fn next_serial(ctx: &mut Ctx) -> u32 {
    let s = ctx.ext.slot::<Serial>("filters.serial");
    s.0 += 1;
    s.0
}

/// Whether a factory by exactly this name is registered.
fn has_factory(ctx: &mut Ctx, name: &str) -> bool {
    FACTORIES.contains(&name) || crate::user_filters::is_registered(ctx, name)
}

/// Whether `stream_filter_register()` may take this name.
pub(crate) fn name_taken(ctx: &mut Ctx, name: &str) -> bool {
    has_factory(ctx, name)
}

/// Build a filter the way php's `php_stream_filter_create` does: the exact
/// name first, then `a.b.*`, `a.*` for `a.b.c`, trying each factory found
/// until one succeeds. `params` is `None` when the caller gave none (an
/// explicit `null` is `Some(Null)`, which the `convert.*` factory rejects).
/// The warning names `func`. A factory's exception (a user filter class
/// whose defaults fail) is raised after php's warnings, as php raises it.
pub(crate) fn create(ctx: &mut Ctx, name: &str, params: Option<&Value>, func: &str) -> Result<Option<Filter>, Unwind> {
    let mut deferred: Option<Unwind> = None;
    let mut found;
    let mut filter = None;
    if has_factory(ctx, name) {
        found = true;
        filter = factory_create(ctx, name, name, params, func, &mut deferred)?;
    } else {
        found = false;
        let mut wild = name.rfind('.').map(|p| name[..p].to_string());
        while let Some(w) = wild.take() {
            let key = format!("{w}.*");
            found = has_factory(ctx, &key);
            if found {
                filter = factory_create(ctx, &key, name, params, func, &mut deferred)?;
                if filter.is_some() {
                    break;
                }
            }
            wild = w.rfind('.').map(|p| w[..p].to_string());
        }
    }
    if filter.is_none() {
        let what = if found { "Unable to create or locate filter" } else { "Unable to locate filter" };
        ctx.warn(&format!("{func}(): {what} \"{name}\""))?;
    }
    match deferred {
        Some(u) => Err(u),
        None => Ok(filter),
    }
}

/// One factory's attempt at `name`.
fn factory_create(
    ctx: &mut Ctx,
    factory: &str,
    name: &str,
    params: Option<&Value>,
    func: &str,
    deferred: &mut Option<Unwind>,
) -> Result<Option<Filter>, Unwind> {
    if FACTORIES.contains(&factory) {
        return builtin_create(ctx, factory, name, params, func);
    }
    crate::user_filters::create(ctx, name, params, func, deferred)
}

/// The built-in factories.
fn builtin_create(
    ctx: &mut Ctx,
    factory: &str,
    name: &str,
    params: Option<&Value>,
    func: &str,
) -> Result<Option<Filter>, Unwind> {
    Ok(match factory {
        "dechunk" => Some(Filter::Dechunk(crate::dechunk::Dechunk::default())),
        "string.rot13" => Some(Filter::Rot13),
        "string.toupper" => Some(Filter::ToUpper),
        "string.tolower" => Some(Filter::ToLower),
        "consumed" => Some(Filter::Consumed(0)),
        "convert.iconv.*" => convert::Iconv::named(name).map(|f| Filter::Native(Box::new(f))),
        "convert.*" => convert_create(ctx, name, params, func)?,
        _ => None,
    })
}

/// php's `strfilter_convert_create`: the mode after the first `.`, in any
/// case, and its options out of the `$params` array.
fn convert_create(ctx: &mut Ctx, name: &str, params: Option<&Value>, func: &str) -> Result<Option<Filter>, Unwind> {
    let opts = match params.map(|v| v.deref().into_owned()) {
        None => None,
        Some(Value::Array(a)) => Some(a),
        Some(_) => {
            ctx.warn(&format!("{func}(): Stream filter ({name}): invalid filter parameter"))?;
            return Ok(None);
        }
    };
    let Some(mode) = name.split_once('.').map(|(_, m)| m.to_ascii_lowercase()) else {
        return Ok(None);
    };
    let str_opt = |ctx: &mut Ctx, key: &str| -> Result<Option<Vec<u8>>, Unwind> {
        let Some(v) = opts.as_ref().and_then(|a| a.get(&ArrayKey::str(key.as_bytes()))) else {
            return Ok(None);
        };
        let v = v.deref().into_owned();
        if matches!(v, Value::Array(_)) {
            ctx.warn("Array to string conversion")?;
            return Ok(Some(b"Array".to_vec()));
        }
        Ok(Some(v.to_php_bytes().to_vec()))
    };
    let get = |key: &str| opts.as_ref().and_then(|a| a.get(&ArrayKey::str(key.as_bytes()))).map(|v| v.deref().into_owned());
    // php reads `line-break-chars` before `line-length`, and drops the
    // break when the line is shorter than one base64 quantum.
    let line_opts = |ctx: &mut Ctx| -> Result<convert::Options, Unwind> {
        let lb = str_opt(ctx, "line-break-chars")?;
        let line_len = get("line-length").map_or(0, |v| v.to_int().clamp(0, u32::MAX as i64) as u32);
        let lbchars = if line_len < 4 { None } else { Some(lb.unwrap_or_else(|| b"\r\n".to_vec())) };
        Ok(convert::Options { line_len, lbchars, binary: false, force_encode_first: false })
    };
    let f: Box<dyn NativeFilter> = match mode.as_str() {
        "base64-encode" => Box::new(convert::Base64Encode::new(line_opts(ctx)?)),
        "base64-decode" => Box::new(convert::Base64Decode::new(name)),
        "quoted-printable-encode" => {
            let mut o = line_opts(ctx)?;
            o.binary = get("binary").is_some_and(|v| v.to_bool());
            o.force_encode_first = get("force-encode-first").is_some_and(|v| v.to_bool());
            Box::new(convert::QpEncode::new(o))
        }
        "quoted-printable-decode" => Box::new(convert::QpDecode::new(name, str_opt(ctx, "line-break-chars")?)),
        _ => return Ok(None),
    };
    Ok(Some(Filter::Native(f)))
}

// ---- running a chain -----------------------------------------------------------------

/// Run `chain` over the run's input: the first filter with `first`, the
/// rest with `rest` (the same flags for a write or a read; php's flush of
/// one filter passes `Normal` on to the ones after it). `consumed` is the
/// first filter's count, where the caller wants it.
fn run_chain(ctx: &mut Ctx, chain: &mut [Attached], run: Run<'_>, func: &str) -> Result<(Status, Vec<Vec<u8>>), Unwind> {
    let Run { mut input, first, rest, mut consumed, stream_prop, .. } = run;
    let mut flush = first;
    for (i, a) in chain.iter_mut().enumerate() {
        let c = if i == 0 { consumed.as_deref_mut() } else { None };
        let (status, out) = a.filter.run(ctx, &stream_prop, input, c, flush, func)?;
        if status != Status::PassOn {
            return Ok((status, Vec::new()));
        }
        input = out;
        flush = rest;
    }
    Ok((Status::PassOn, input))
}

/// Which chain of a stream.
#[derive(Clone, Copy, PartialEq)]
enum Side {
    Read,
    Write,
}

/// What `run_taken` hands the chain.
struct Run<'a> {
    side: Side,
    /// Where in the chain to start (a removal flushes from the filter out).
    from: usize,
    input: Vec<Vec<u8>>,
    first: Flush,
    rest: Flush,
    consumed: Option<&'a mut i64>,
    /// What `$this->stream` is during the call: the stream, or `null` for
    /// a stream being torn down at the end of the request.
    stream_prop: Value,
}

/// Take a stream's chain out of it for the length of the run — a user
/// filter is script code, and may touch the stream — and put it back, with
/// anything attached meanwhile after it.
fn run_taken(ctx: &mut Ctx, v: &Value, func: &str, run: Run<'_>) -> Result<(Status, Vec<Vec<u8>>), Unwind> {
    let side = run.side;
    let mut chain = crate::file::with_stream_mut(ctx, v, func, |s| {
        s.fstate.busy = true;
        std::mem::take(if side == Side::Read { &mut s.read_filters } else { &mut s.write_filters })
    })?;
    let from = run.from.min(chain.len());
    let r = run_chain(ctx, &mut chain[from..], run, func);
    let _ = crate::file::with_stream_mut(ctx, v, func, |s| {
        s.fstate.busy = false;
        let slot = if side == Side::Read { &mut s.read_filters } else { &mut s.write_filters };
        let added = std::mem::take(slot);
        *slot = chain;
        slot.extend(added);
    });
    r
}

fn has_chain(ctx: &mut Ctx, v: &Value, func: &str, side: Side) -> Result<bool, Unwind> {
    crate::file::with_stream_mut(ctx, v, func, |s| {
        !(if side == Side::Read { &s.read_filters } else { &s.write_filters }).is_empty()
    })
}

// ---- the write side ------------------------------------------------------------------

/// A write through the stream's write chain: `None` when there is no chain
/// (the caller writes as usual), else what `fwrite()` answers — the first
/// filter's consumed count, or `false` on a fatal filter.
pub(crate) fn write_through(ctx: &mut Ctx, v: &Value, func: &str, data: &[u8]) -> Result<Option<Value>, Unwind> {
    if !crate::file::with_stream_mut(ctx, v, func, |s| !s.write_filters.is_empty() && s.is_writable())? {
        return Ok(None);
    }
    if data.is_empty() {
        return Ok(Some(Value::Int(0)));
    }
    let mut consumed = 0i64;
    let run = Run {
        side: Side::Write,
        from: 0,
        input: vec![data.to_vec()],
        first: Flush::Normal,
        rest: Flush::Normal,
        consumed: Some(&mut consumed),
        stream_prop: v.clone(),
    };
    let (status, out) = run_taken(ctx, v, func, run)?;
    let answer = match status {
        Status::Fatal => Value::Bool(false),
        Status::PassOn => {
            for b in &out {
                crate::file::write_raw(ctx, v, b)?;
            }
            Value::Int(consumed)
        }
        _ => Value::Int(consumed),
    };
    rethrow(ctx)?;
    Ok(Some(answer))
}

/// A user filter's exception, held until the stream operation it broke
/// has finished the way php finishes it (`user_filters.rs`).
#[derive(Default)]
struct Deferred(Option<Unwind>);

pub(crate) fn defer(ctx: &mut Ctx, u: Unwind) {
    let d = ctx.ext.slot::<Deferred>("filters.deferred");
    if d.0.is_none() {
        d.0 = Some(u);
    }
}

pub(crate) fn has_deferred(ctx: &mut Ctx) -> bool {
    ctx.ext.slot::<Deferred>("filters.deferred").0.is_some()
}

/// Raise the held exception, if there is one.
pub(crate) fn rethrow(ctx: &mut Ctx) -> Result<(), Unwind> {
    match ctx.ext.slot::<Deferred>("filters.deferred").0.take() {
        Some(u) => Err(u),
        None => Ok(()),
    }
}

/// Flush the write chain (`fflush()`, a seek, a close): php's
/// `_php_stream_flush`, which runs the whole chain with the flag and
/// writes what comes out. `false` when a filter failed.
pub(crate) fn flush_writes(ctx: &mut Ctx, v: &Value, func: &str, flush: Flush, stream_prop: Value) -> Result<bool, Unwind> {
    if !has_chain(ctx, v, func, Side::Write)? {
        return Ok(true);
    }
    let mut consumed = 0i64;
    let run = Run {
        side: Side::Write,
        from: 0,
        input: Vec::new(),
        first: flush,
        rest: flush,
        consumed: Some(&mut consumed),
        stream_prop,
    };
    let (status, out) = run_taken(ctx, v, func, run)?;
    for b in &out {
        crate::file::write_raw(ctx, v, b)?;
    }
    Ok(status != Status::Fatal)
}

// ---- the read side -------------------------------------------------------------------

/// How much a read wants in the view before it runs.
pub(crate) enum Want {
    /// At least this many bytes (`fread`, `fgetc`).
    Bytes(usize),
    /// Up to and including this delimiter, or `cap` bytes (`fgets`,
    /// `stream_get_line`).
    Line(Vec<u8>, usize),
    /// Everything (`stream_get_contents`).
    All,
}

/// Fill the view for a read on a buffer-backed stream with a read chain:
/// php's `_php_stream_fill_read_buffer`, a chunk at a time until the read
/// is satisfied or the underlying stream runs dry, where the chain gets
/// its closing call. Answers whether a filter failed — `fread()` is then
/// `false` rather than empty.
pub(crate) fn fill(ctx: &mut Ctx, v: &Value, func: &str, want: Want) -> Result<bool, Unwind> {
    let failed = fill_view(ctx, v, func, want)?;
    rethrow(ctx)?;
    Ok(failed)
}

fn fill_view(ctx: &mut Ctx, v: &Value, func: &str, want: Want) -> Result<bool, Unwind> {
    loop {
        let probe = crate::file::with_stream_mut(ctx, v, func, |s| {
            if s.read_filters.is_empty() || s.is_pipe() {
                return None;
            }
            let avail = s.fstate.avail();
            let target = match &want {
                Want::Bytes(n) => *n,
                Want::All => usize::MAX,
                Want::Line(delim, cap) => {
                    let found = !delim.is_empty() && avail.windows(delim.len()).any(|w| w == &delim[..]);
                    if found || avail.len() >= *cap {
                        0
                    } else {
                        // php copies out what it has and asks for a chunk more.
                        avail.len() + CHUNK
                    }
                }
            };
            Some((avail.len(), target, s.fstate.done))
        })?;
        let Some((avail, target, done)) = probe else { return Ok(false) };
        if avail >= target || done {
            return Ok(false);
        }
        let mut have = avail;
        while have < target {
            let raw = crate::file::with_stream_mut(ctx, v, func, |s| s.take_raw(CHUNK))?;
            let flush = if raw.is_empty() { Flush::Close } else { Flush::Normal };
            let input = if raw.is_empty() { Vec::new() } else { vec![raw.clone()] };
            let run = Run { side: Side::Read, from: 0, input, first: flush, rest: flush, consumed: None, stream_prop: v.clone() };
            let (status, out) = run_taken(ctx, v, func, run)?;
            let failed = status == Status::Fatal;
            crate::file::with_stream_mut(ctx, v, func, |s| {
                for b in &out {
                    s.fstate.push_view(b);
                }
                if raw.is_empty() || failed {
                    s.fstate.done = true;
                }
                have = s.fstate.avail().len();
            })?;
            if failed {
                return Ok(true);
            }
            if raw.is_empty() {
                break;
            }
        }
        if matches!(want, Want::Bytes(_) | Want::All) {
            return Ok(false);
        }
    }
}

/// A read off a descriptor, through the read chain: the chunk that came
/// off it is one bucket, and the read that found the end is the chain's
/// closing call.
pub(crate) fn filter_pipe_read(ctx: &mut Ctx, v: &Value, func: &str, raw: Vec<u8>, at_end: bool) -> Result<Vec<u8>, Unwind> {
    let closing = crate::file::with_stream_mut(ctx, v, func, |s| {
        let closing = at_end && !s.fstate.done;
        if closing {
            s.fstate.done = true;
        }
        closing
    })?;
    let mut produced = Vec::new();
    // What came off the descriptor, then — the read that found the end —
    // the closing call.
    for (input, flush) in [(raw, Flush::Normal), (Vec::new(), Flush::Close)] {
        if flush == Flush::Normal && input.is_empty() || flush == Flush::Close && !closing {
            continue;
        }
        let input = if input.is_empty() { Vec::new() } else { vec![input] };
        let run = Run { side: Side::Read, from: 0, input, first: flush, rest: flush, consumed: None, stream_prop: v.clone() };
        let (status, out) = run_taken(ctx, v, func, run)?;
        if status == Status::PassOn {
            produced.extend(out.concat());
        }
    }
    rethrow(ctx)?;
    Ok(produced)
}

// ---- attaching, removing, closing ----------------------------------------------------

/// What a `stream filter` resource carries: which stream it is on and
/// which attachment it stands for.
pub(crate) struct FilterHandle {
    pub(crate) stream: Value,
    pub(crate) id: u32,
    pub(crate) read: bool,
}

/// `stream_filter_append(resource $stream, string $filtername, int $mode = 0, mixed $params = null): resource|false`
fn stream_filter_append(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let r = attach(ctx, args, "stream_filter_append", false)?;
    rethrow(ctx)?;
    Ok(r)
}

/// `stream_filter_prepend(resource $stream, string $filtername, int $mode = 0, mixed $params = null): resource|false`
fn stream_filter_prepend(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let r = attach(ctx, args, "stream_filter_prepend", true)?;
    rethrow(ctx)?;
    Ok(r)
}

/// The stream argument, or php's TypeError.
fn open_stream_arg(v: &Value, func: &str) -> Result<Value, Unwind> {
    match &*v.deref() {
        Value::Resource(r) if r.kind() == "stream" => Ok(Value::Resource(r.clone())),
        Value::Resource(_) => Err(Unwind::type_error(format!(
            "{func}(): Argument #1 ($stream) must be an open stream resource"
        ))),
        other => Err(Unwind::type_error(format!(
            "{func}(): Argument #1 ($stream) must be of type resource, {} given",
            rphp_runtime::value_name(other)
        ))),
    }
}

fn attach(ctx: &mut Ctx, args: &mut [Value], func: &str, first: bool) -> NativeResult {
    let stream = open_stream_arg(&args[0], func)?;
    let name = String::from_utf8_lossy(&args[1].to_php_bytes()).into_owned();
    let mut mode = args.get(2).map_or(0, Value::to_int);
    let params = args.get(3).map(|v| v.deref().into_owned());
    // php's default is whatever the stream was opened for.
    if mode & (READ | WRITE) == 0 {
        let m = crate::file::with_stream_mut(ctx, &stream, func, |s| s.mode_str().to_string())?;
        mode = 0;
        if m.contains('r') || m.contains('+') {
            mode |= READ;
        }
        if m.contains('w') || m.contains('+') || m.contains('a') {
            mode |= WRITE;
        }
    }
    let mut last = None;
    for (bit, read) in [(READ, true), (WRITE, false)] {
        if mode & bit == 0 {
            continue;
        }
        let Some(filter) = create(ctx, &name, params.as_ref(), func)? else {
            return Ok(Value::Bool(false));
        };
        let id = next_serial(ctx);
        if !attach_one(ctx, &stream, func, Attached { id, res: None, filter }, read, first)? {
            return Ok(Value::Bool(false));
        }
        last = Some((id, read));
    }
    let Some((id, read)) = last else { return Ok(Value::Bool(false)) };
    let handle = ctx.resources.add("stream filter", Box::new(FilterHandle { stream: stream.clone(), id, read }));
    if let Value::Resource(r) = &handle {
        let res = r.id();
        crate::file::with_stream_mut(ctx, &stream, func, |s| {
            let chain = if read { &mut s.read_filters } else { &mut s.write_filters };
            if let Some(a) = chain.iter_mut().find(|a| a.id == id) {
                a.res = Some(res);
            }
        })?;
    }
    Ok(handle)
}

/// Put one filter on a chain. An appended read filter first gets what the
/// stream already holds for reads — php's "pre-buffered data" — on its own.
fn attach_one(ctx: &mut Ctx, v: &Value, func: &str, a: Attached, read: bool, first: bool) -> Result<bool, Unwind> {
    let id = a.id;
    let pre = crate::file::with_stream_mut(ctx, v, func, |s| {
        let pre = if read && !first && !s.is_pipe() {
            if s.fstate.view_active(!s.read_filters.is_empty()) {
                let rest = s.fstate.avail().to_vec();
                let pos = s.fstate.vbase + s.fstate.vpos;
                s.fstate.reset_view(pos);
                Some(rest)
            } else {
                let pos = s.logical_pos();
                s.fstate.reset_view(pos);
                Some(s.take_prebuffered())
            }
        } else {
            None
        };
        let chain = if read { &mut s.read_filters } else { &mut s.write_filters };
        if first {
            chain.insert(0, a);
        } else {
            chain.push(a);
        }
        pre
    })?;
    let Some(pre) = pre.filter(|p| !p.is_empty()) else { return Ok(true) };
    let len = pre.len() as i64;
    let mut consumed = 0i64;
    let from = crate::file::with_stream_mut(ctx, v, func, |s| s.read_filters.len() - 1)?;
    let run = Run {
        side: Side::Read,
        from,
        input: vec![pre],
        first: Flush::Normal,
        rest: Flush::Normal,
        consumed: Some(&mut consumed),
        stream_prop: v.clone(),
    };
    let (mut status, out) = run_taken(ctx, v, func, run)?;
    if consumed > len {
        // "No behaving filter should cause this."
        status = Status::Fatal;
    }
    if status == Status::Fatal {
        ctx.warn(&format!("{func}(): Filter failed to process pre-buffered data"))?;
        let removed = take_attached(ctx, v, func, id, true)?;
        if let Some(mut a) = removed {
            a.filter.dispose(ctx)?;
        }
        return Ok(false);
    }
    if status == Status::PassOn {
        crate::file::with_stream_mut(ctx, v, func, |s| {
            for b in &out {
                s.fstate.push_view(b);
            }
        })?;
    }
    Ok(true)
}

/// Unlink one attachment from its chain.
fn take_attached(ctx: &mut Ctx, v: &Value, func: &str, id: u32, read: bool) -> Result<Option<Attached>, Unwind> {
    crate::file::with_stream_mut(ctx, v, func, |s| {
        let chain = if read { &mut s.read_filters } else { &mut s.write_filters };
        chain.iter().position(|a| a.id == id).map(|i| chain.remove(i))
    })
}

/// `stream_filter_remove(resource $stream_filter): bool`
fn stream_filter_remove(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    const FUNC: &str = "stream_filter_remove";
    let Value::Resource(r) = &*args[0].deref() else {
        return Err(Unwind::type_error(format!(
            "{FUNC}(): Argument #1 ($stream_filter) must be of type resource, {} given",
            rphp_runtime::value_name(&args[0].deref())
        )));
    };
    let handle = if r.kind() == "stream filter" {
        let mut payload = r.payload_mut();
        payload.as_mut().and_then(|a| a.downcast_mut::<FilterHandle>()).map(|h| (h.stream.clone(), h.id, h.read))
    } else {
        None
    };
    let Some((stream, id, read)) = handle else {
        return Err(Unwind::type_error(format!(
            "{FUNC}(): supplied resource is not a valid stream filter resource"
        )));
    };
    let res = r.id();
    // php flushes the filter, and everything after it, before unlinking it:
    // what it held back goes where the chain's output goes.
    let side = if read { Side::Read } else { Side::Write };
    let at = crate::file::with_stream_mut(ctx, &stream, FUNC, |s| {
        let chain = if read { &s.read_filters } else { &s.write_filters };
        chain.iter().position(|a| a.id == id)
    })?;
    let Some(at) = at else {
        return Ok(Value::Bool(false));
    };
    let run = Run {
        side,
        from: at,
        input: Vec::new(),
        first: Flush::Close,
        rest: Flush::Normal,
        consumed: None,
        stream_prop: stream.clone(),
    };
    let (status, out) = run_taken(ctx, &stream, FUNC, run)?;
    if status == Status::Fatal {
        ctx.warn(&format!("{FUNC}(): Unable to flush filter, not removing"))?;
        rethrow(ctx)?;
        return Ok(Value::Bool(false));
    }
    if status == Status::PassOn {
        for b in &out {
            if read {
                crate::file::with_stream_mut(ctx, &stream, FUNC, |s| s.fstate.push_view(b))?;
            } else {
                crate::file::write_raw(ctx, &stream, b)?;
            }
        }
    }
    ctx.resources.close(res);
    if let Some(mut a) = take_attached(ctx, &stream, FUNC, id, read)? {
        a.filter.dispose(ctx)?;
    }
    rethrow(ctx)?;
    Ok(Value::Bool(true))
}

/// A stream is closing (`fclose()`, or the end of the request with
/// `stream_prop` null): the write chain's closing flush, then every
/// filter's `onClose()` — the read chain's first, as php frees them — and
/// their resources closed.
pub(crate) fn close_stream(ctx: &mut Ctx, v: &Value, func: &str, stream_prop: Value) -> Result<(), Unwind> {
    let any = crate::file::with_stream_mut(ctx, v, func, |s| !s.read_filters.is_empty() || !s.write_filters.is_empty())?;
    if !any {
        return Ok(());
    }
    flush_writes(ctx, v, func, Flush::Close, stream_prop)?;
    let (reads, writes) = crate::file::with_stream_mut(ctx, v, func, |s| {
        (std::mem::take(&mut s.read_filters), std::mem::take(&mut s.write_filters))
    })?;
    for mut a in reads.into_iter().chain(writes) {
        if let Some(res) = a.res {
            ctx.resources.close(res);
        }
        a.filter.dispose(ctx)?;
    }
    Ok(())
}

/// `stream_get_filters(): array`
fn stream_get_filters(ctx: &mut Ctx, _args: &mut [Value]) -> NativeResult {
    let mut out = Array::new();
    for f in FACTORIES {
        out.push(Value::string(f.as_bytes()));
    }
    for name in crate::user_filters::registered_names(ctx) {
        out.push(Value::string(name.as_bytes()));
    }
    Ok(Value::Array(out))
}

// ---- php://filter ----------------------------------------------------------------------

/// Attach a `php://filter` url's filters to the stream opened for its
/// `resource=`. `spec` is what php tokenises: the url after `php://filter`
/// with the `/resource=` cut off — php cuts it with a NUL and starts its
/// scan one byte in, so `php://filter/resource=x` scans the resource
/// itself (a php quirk kept here). Segments are `read=a|b`, `write=c`, or
/// a bare list for the directions the stream was opened in; each name is
/// url-decoded.
pub(crate) fn apply_url_filters(ctx: &mut Ctx, v: &Value, spec: &[u8], mode: &str, func: &str) -> Result<(), Unwind> {
    let can_read = mode.contains('r') || mode.contains('+');
    let can_write = mode.contains('w') || mode.contains('+') || mode.contains('a');
    let mut pending: Option<Unwind> = None;
    for seg in spec.split(|b| *b == b'/').filter(|s| !s.is_empty()) {
        let seg = url_decode(seg);
        let (list, read, write): (&[u8], bool, bool) = if seg.len() >= 5 && seg[..5].eq_ignore_ascii_case(b"read=") {
            (&seg[5..], true, false)
        } else if seg.len() >= 6 && seg[..6].eq_ignore_ascii_case(b"write=") {
            (&seg[6..], false, true)
        } else {
            (&seg[..], can_read, can_write)
        };
        for name in list.split(|b| *b == b'|').filter(|s| !s.is_empty()) {
            let name = String::from_utf8_lossy(&url_decode(name)).into_owned();
            for (on, is_read) in [(read, true), (write, false)] {
                if !on {
                    continue;
                }
                match create(ctx, &name, None, func) {
                    Ok(Some(filter)) => {
                        let id = next_serial(ctx);
                        attach_one(ctx, v, func, Attached { id, res: None, filter }, is_read, false)?;
                    }
                    Ok(None) => ctx.warn(&format!("{func}(): Unable to create filter ({name})"))?,
                    // php finishes the list with the exception pending, then
                    // closes the stream and lets it through.
                    Err(u) => {
                        ctx.warn(&format!("{func}(): Unable to create filter ({name})"))?;
                        pending.get_or_insert(u);
                    }
                }
            }
        }
    }
    match pending {
        Some(u) => {
            ctx.call_function(b"fclose", std::slice::from_ref(v))?;
            Err(u)
        }
        None => rethrow(ctx),
    }
}

/// php's `php_url_decode`: `+` is a space and `%XX` a byte.
fn url_decode(s: &[u8]) -> Vec<u8> {
    let hex = |b: u8| (b as char).to_digit(16).map(|d| d as u8);
    let mut out = Vec::with_capacity(s.len());
    let mut i = 0;
    while i < s.len() {
        match (s[i], s.get(i + 1).copied().and_then(hex), s.get(i + 2).copied().and_then(hex)) {
            (b'+', _, _) => out.push(b' '),
            (b'%', Some(h), Some(l)) => {
                out.push(h << 4 | l);
                i += 2;
            }
            (b, _, _) => out.push(b),
        }
        i += 1;
    }
    out
}
