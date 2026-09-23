//! User stream filters (php-src `ext/standard/user_filters.c`):
//! `stream_filter_register()`, the `php_user_filter` base class, the
//! `StreamBucket` class and the `stream_bucket_*` functions.
//!
//! A registered name maps to a class; attaching a filter by that name (or
//! by one its `prefix.*` registration covers) makes an instance — without
//! running its constructor, as php's `object_init_ex` does — with
//! `filtername` and `params` set, and asks `onCreate()`, whose `false`
//! refuses the attachment. Every time the stream's chain runs, the object's
//! `filter($in, $out, &$consumed, $closing)` gets two fresh *bucket
//! brigade* resources: it takes buckets off `$in` with
//! `stream_bucket_make_writeable()`, changes their `data`, and hands them on
//! with `stream_bucket_append()`/`stream_bucket_prepend()` to `$out`, or
//! makes new ones with `stream_bucket_new()`. `$this->stream` is the stream
//! for the length of the call. What it leaves on `$in` is php's
//! "Unprocessed filter buckets" warning; what it answers other than
//! `PSFS_PASS_ON` throws `$out` away. `onClose()` runs when the filter is
//! removed or its stream closed.
//!
//! The brigades and buckets are real resources, as php's are, so resource
//! ids later in the script count the same as php's.
//!
//! The registry is per request (`Interp::ext`), so nothing here needs a
//! reset between requests.

use rphp_runtime::{nf, nm, ClassFlags, Ctx, NativeFn, NativeMethod, NativeResult, Registry, Unwind, Visibility};
use rphp_value::{Object, PhpRef, Value};

use crate::filters::{Filter, Status};

/// This module's registry contribution (see `lib.rs`).
pub(crate) static FUNCTIONS: &[NativeFn] = &[
    nf!("stream_filter_register", 2, Some(2), stream_filter_register),
    nf!("stream_bucket_make_writeable", 1, Some(1), stream_bucket_make_writeable),
    nf!("stream_bucket_append", 2, Some(2), stream_bucket_append),
    nf!("stream_bucket_prepend", 2, Some(2), stream_bucket_prepend),
    nf!("stream_bucket_new", 2, Some(2), stream_bucket_new),
];

/// php's resource type names.
const BRIGADE: &str = "userfilter.bucket brigade";
const BUCKET: &str = "userfilter.bucket";

pub(crate) fn register_classes(r: &mut Registry) {
    if r.interp().class_by_name(b"php_user_filter").is_some() {
        return;
    }
    r.class("php_user_filter")
        .prop("filtername", Visibility::Public, Value::string(b""))
        .prop("params", Visibility::Public, Value::string(b""))
        .prop("stream", Visibility::Public, Value::Null)
        .method(
            "filter",
            NativeMethod {
                params: &["in", "out", "consumed", "closing"],
                by_ref: 1 << 2,
                ..nm!(4, Some(4), filter_method)
            },
        )
        .method("onCreate", nm!(0, Some(0), on_create_method))
        .method("onClose", nm!(0, Some(0), on_close_method))
        .finish();
    r.class("StreamBucket")
        .flags(ClassFlags::FINAL)
        .prop("bucket", Visibility::Public, Value::Null)
        .prop("data", Visibility::Public, Value::string(b""))
        .prop("datalen", Visibility::Public, Value::Int(0))
        .prop("dataLength", Visibility::Public, Value::Int(0))
        .finish();
}

pub(crate) fn register_constants(r: &mut Registry) {
    for (name, v) in [
        ("PSFS_PASS_ON", 2),
        ("PSFS_FEED_ME", 1),
        ("PSFS_ERR_FATAL", 0),
        ("PSFS_FLAG_NORMAL", 0),
        ("PSFS_FLAG_FLUSH_INC", 1),
        ("PSFS_FLAG_FLUSH_CLOSE", 2),
    ] {
        r.constant(name, Value::Int(v));
    }
}

// ---- the registry ----------------------------------------------------------------------

/// The names `stream_filter_register()` took this request, in order, with
/// the class each one names.
#[derive(Default)]
struct Registered(Vec<(String, Vec<u8>)>);

fn registered<'a>(ctx: &'a mut Ctx<'_>) -> &'a mut Vec<(String, Vec<u8>)> {
    &mut ctx.ext.slot::<Registered>("user_filters.map").0
}

pub(crate) fn is_registered(ctx: &mut Ctx, name: &str) -> bool {
    registered(ctx).iter().any(|(n, _)| n == name)
}

/// The registered names, in the order `stream_get_filters()` lists them.
pub(crate) fn registered_names(ctx: &mut Ctx) -> Vec<String> {
    registered(ctx).iter().map(|(n, _)| n.clone()).collect()
}

/// The class a filter name resolves to: the exact registration, else the
/// nearest `prefix.*` one, as php's user factory looks it up again itself.
fn class_for(ctx: &mut Ctx, name: &str) -> Option<Vec<u8>> {
    let map = registered(ctx);
    let find = |key: &str| map.iter().find(|(n, _)| n == key).map(|(_, c)| c.clone());
    if let Some(c) = find(name) {
        return Some(c);
    }
    let mut stem = &name[..name.rfind('.')?];
    loop {
        if let Some(c) = find(&format!("{stem}.*")) {
            return Some(c);
        }
        stem = &stem[..stem.rfind('.')?];
    }
}

/// `stream_filter_register(string $filter_name, string $class): bool`
fn stream_filter_register(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    const FUNC: &str = "stream_filter_register";
    let name = str_arg(&args[0], FUNC, 1, "filter_name")?;
    let class = str_arg(&args[1], FUNC, 2, "class")?;
    if name.is_empty() {
        return Err(Unwind::value_error(format!("{FUNC}(): Argument #1 ($filter_name) must be a non-empty string")));
    }
    if class.is_empty() {
        return Err(Unwind::value_error(format!("{FUNC}(): Argument #2 ($class) must be a non-empty string")));
    }
    let name = String::from_utf8_lossy(&name).into_owned();
    if crate::filters::name_taken(ctx, &name) {
        return Ok(Value::Bool(false));
    }
    registered(ctx).push((name, class));
    Ok(Value::Bool(true))
}

/// A `string` parameter, php's TypeError for what cannot be one.
fn str_arg(v: &Value, func: &str, n: usize, name: &str) -> Result<Vec<u8>, Unwind> {
    match &*v.deref() {
        Value::Array(_) | Value::Object(_) | Value::Closure(_) | Value::Resource(_) => Err(Unwind::type_error(format!(
            "{func}(): Argument #{n} (${name}) must be of type string, {} given",
            rphp_runtime::value_name(&v.deref())
        ))),
        other => Ok(other.to_php_bytes().to_vec()),
    }
}

/// php's user filter factory: an instance of the registered class, told
/// its name and parameters, and asked `onCreate()`. An exception out of the
/// class's property defaults is `deferred`: php reports the failed filter
/// first and raises it after.
pub(crate) fn create(
    ctx: &mut Ctx,
    name: &str,
    params: Option<&Value>,
    func: &str,
    deferred: &mut Option<Unwind>,
) -> Result<Option<Filter>, Unwind> {
    let Some(class) = class_for(ctx, name) else { return Ok(None) };
    let Some(cid) = ctx.lookup_class(&class)? else {
        ctx.warn(&format!(
            "{func}(): User-filter \"{name}\" requires class \"{}\", but that class is not defined",
            String::from_utf8_lossy(&class)
        ))?;
        return Ok(None);
    };
    let obj = match ctx.new_object(cid) {
        Ok(o) => o,
        Err(u) => {
            *deferred = Some(u);
            return Ok(None);
        }
    };
    obj.with_data_mut(|d| {
        d.set(b"filtername", Value::string(name.as_bytes()));
        d.set(b"params", params.cloned().unwrap_or(Value::Null));
    });
    if matches!(ctx.call_method(&obj, b"onCreate", &[])?, Value::Bool(false)) {
        return Ok(None);
    }
    Ok(Some(Filter::User(UserFilter { obj })))
}

// ---- the filter ------------------------------------------------------------------------

/// An attached user filter: the object whose methods do the work.
pub(crate) struct UserFilter {
    obj: Object,
}

/// A brigade's buckets: each with the id of the `userfilter.bucket`
/// resource that stands for it (`0` for one that never had one), so the
/// same bucket appended twice is linked once, as in php's list.
#[derive(Default)]
struct Brigade(Vec<(u32, Vec<u8>)>);

/// A bucket resource's payload.
struct Bucket(Vec<u8>);

impl UserFilter {
    /// php's `userfilter_filter`.
    pub(crate) fn call(
        &mut self,
        ctx: &mut Ctx,
        stream: &Value,
        input: Vec<Vec<u8>>,
        consumed: Option<&mut i64>,
        closing: bool,
        func: &str,
    ) -> Result<(Status, Vec<Vec<u8>>), Unwind> {
        self.obj.with_data_mut(|d| d.set(b"stream", stream.clone()));
        let brigade_in = ctx.resources.add(BRIGADE, Box::new(Brigade(input.into_iter().map(|b| (0, b)).collect())));
        let brigade_out = ctx.resources.add(BRIGADE, Box::new(Brigade::default()));
        let cell = PhpRef::new(consumed.as_ref().map_or(Value::Null, |c| Value::Int(**c)));
        let args = [brigade_in.clone(), brigade_out.clone(), Value::Ref(cell.clone()), Value::Bool(closing)];
        // With an exception already pending php does not call into the
        // script again; the call fails.
        let r = if crate::filters::has_deferred(ctx) {
            Ok(Value::Int(0))
        } else {
            ctx.call_method(&self.obj, b"filter", &args)
        };
        let status = match &r {
            Ok(v) => match v.to_int() {
                2 => Status::PassOn,
                1 => Status::FeedMe,
                0 => Status::Fatal,
                _ => Status::Other,
            },
            Err(_) => Status::Fatal,
        };
        // An exception out of the method is php's pending exception: the
        // stream operation finishes as a failed filter call, warnings and
        // all, and the exception surfaces when it returns.
        if let Err(u) = r {
            crate::filters::defer(ctx, u);
        }
        if let Some(c) = consumed {
            *c = cell.get().to_int();
        }
        let left = take_brigade(&brigade_in);
        let out = take_brigade(&brigade_out);
        ctx.resources.close_value(&brigade_in);
        ctx.resources.close_value(&brigade_out);
        self.obj.with_data_mut(|d| d.set(b"stream", Value::Null));
        if !left.is_empty() {
            ctx.warn(&format!("{func}(): Unprocessed filter buckets remaining on input brigade"))?;
        }
        if status != Status::PassOn {
            return Ok((status, Vec::new()));
        }
        Ok((status, out))
    }

    /// php's `userfilter_dtor`: `onClose()`.
    pub(crate) fn on_close(&mut self, ctx: &mut Ctx) -> Result<(), Unwind> {
        if crate::filters::has_deferred(ctx) {
            return Ok(());
        }
        ctx.call_method(&self.obj, b"onClose", &[]).map(|_| ())
    }
}

/// Empty a brigade resource, answering its buckets' bytes.
fn take_brigade(v: &Value) -> Vec<Vec<u8>> {
    let Value::Resource(r) = v else { return Vec::new() };
    let mut payload = r.payload_mut();
    match payload.as_mut().and_then(|a| a.downcast_mut::<Brigade>()) {
        Some(b) => std::mem::take(&mut b.0).into_iter().map(|(_, d)| d).collect(),
        None => Vec::new(),
    }
}

// ---- the class ---------------------------------------------------------------------------

/// `php_user_filter::filter($in, $out, &$consumed, bool $closing): int` —
/// the base class fails every call (`PSFS_ERR_FATAL`).
fn filter_method(_ctx: &mut Ctx, _this: Option<&Object>, args: &mut [Value]) -> NativeResult {
    for (i, name) in [(0, "in"), (1, "out")] {
        if !matches!(&*args[i].deref(), Value::Resource(_)) {
            return Err(Unwind::type_error(format!(
                "php_user_filter::filter(): Argument #{} (${name}) must be of type resource, {} given",
                i + 1,
                rphp_runtime::value_name(&args[i].deref())
            )));
        }
    }
    Ok(Value::Int(0))
}

/// `php_user_filter::onCreate(): bool`
fn on_create_method(_ctx: &mut Ctx, _this: Option<&Object>, _args: &mut [Value]) -> NativeResult {
    Ok(Value::Bool(true))
}

/// `php_user_filter::onClose(): void`
fn on_close_method(_ctx: &mut Ctx, _this: Option<&Object>, _args: &mut [Value]) -> NativeResult {
    Ok(Value::Null)
}

// ---- buckets -----------------------------------------------------------------------------

/// The brigade argument: a resource of the brigade type, or php's TypeError.
fn brigade_arg(v: &Value, func: &str) -> Result<rphp_value::Resource, Unwind> {
    match &*v.deref() {
        Value::Resource(r) if r.kind() == BRIGADE => Ok(r.clone()),
        Value::Resource(_) => Err(Unwind::type_error(format!(
            "{func}(): supplied resource is not a valid {BRIGADE} resource"
        ))),
        other => Err(Unwind::type_error(format!(
            "{func}(): Argument #1 ($brigade) must be of type resource, {} given",
            rphp_runtime::value_name(other)
        ))),
    }
}

/// A `StreamBucket` for the bytes, with a new bucket resource behind it.
fn bucket_object(ctx: &mut Ctx, data: Vec<u8>) -> Result<Value, Unwind> {
    let Some(cid) = ctx.class_by_name(b"StreamBucket") else {
        return Err(Unwind::error("Class \"StreamBucket\" not found"));
    };
    let len = data.len() as i64;
    let res = ctx.resources.add(BUCKET, Box::new(Bucket(data.clone())));
    let obj = ctx.instantiate(cid);
    obj.with_data_mut(|d| {
        d.set(b"bucket", res);
        d.set(b"data", Value::string(&data));
        d.set(b"datalen", Value::Int(len));
        d.set(b"dataLength", Value::Int(len));
    });
    Ok(Value::Object(obj))
}

/// `stream_bucket_make_writeable(resource $brigade): ?StreamBucket` — the
/// next bucket off the brigade, or `null` when it is empty.
fn stream_bucket_make_writeable(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let r = brigade_arg(&args[0], "stream_bucket_make_writeable")?;
    let head = {
        let mut payload = r.payload_mut();
        match payload.as_mut().and_then(|a| a.downcast_mut::<Brigade>()) {
            Some(b) if !b.0.is_empty() => Some(b.0.remove(0).1),
            _ => None,
        }
    };
    match head {
        Some(data) => bucket_object(ctx, data),
        None => Ok(Value::Null),
    }
}

/// `stream_bucket_append(resource $brigade, StreamBucket $bucket): void`
fn stream_bucket_append(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    attach_bucket(ctx, args, "stream_bucket_append", true)
}

/// `stream_bucket_prepend(resource $brigade, StreamBucket $bucket): void`
fn stream_bucket_prepend(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    attach_bucket(ctx, args, "stream_bucket_prepend", false)
}

fn attach_bucket(ctx: &mut Ctx, args: &mut [Value], func: &str, append: bool) -> NativeResult {
    let _ = ctx;
    // Parameters first, as php's zpp checks them.
    if !matches!(&*args[0].deref(), Value::Resource(_)) {
        brigade_arg(&args[0], func)?;
    }
    let obj = match &*args[1].deref() {
        Value::Object(o) if is_stream_bucket(ctx, o) => o.clone(),
        other => {
            return Err(Unwind::type_error(format!(
                "{func}(): Argument #2 ($bucket) must be of type StreamBucket, {} given",
                rphp_runtime::value_name(other)
            )))
        }
    };
    let Some(handle) = obj.with_data(|d| d.get(b"bucket").map(|v| v.deref().into_owned())) else {
        return Err(Unwind::value_error(format!(
            "{func}(): Argument #2 ($bucket) must be an object that has a \"bucket\" property"
        )));
    };
    let brigade = brigade_arg(&args[0], func)?;
    let bucket = match &handle {
        Value::Resource(b) if b.kind() == BUCKET => b.clone(),
        Value::Resource(_) => {
            return Err(Unwind::type_error(format!("{func}(): supplied resource is not a valid {BUCKET} resource")))
        }
        _ => {
            return Err(Unwind::type_error(format!("{func}(): supplied argument is not a valid {BUCKET} resource")))
        }
    };
    // The object's `data` is what the bucket holds from now on.
    let data = obj.with_data(|d| match d.get(b"data").map(|v| v.deref().into_owned()) {
        Some(Value::Str(s)) => Some(s.as_bytes().to_vec()),
        _ => None,
    });
    let data = {
        let mut payload = bucket.payload_mut();
        let Some(b) = payload.as_mut().and_then(|a| a.downcast_mut::<Bucket>()) else {
            return Ok(Value::Null);
        };
        if let Some(d) = data {
            b.0 = d;
        }
        b.0.clone()
    };
    let id = bucket.id();
    let mut payload = brigade.payload_mut();
    if let Some(b) = payload.as_mut().and_then(|a| a.downcast_mut::<Brigade>()) {
        b.0.retain(|(i, _)| *i != id);
        if append {
            b.0.push((id, data));
        } else {
            b.0.insert(0, (id, data));
        }
    }
    Ok(Value::Null)
}

fn is_stream_bucket(ctx: &mut Ctx, o: &Object) -> bool {
    ctx.class_by_name(b"StreamBucket") == Some(o.class_id())
}

/// `stream_bucket_new(resource $stream, string $buffer): StreamBucket`
fn stream_bucket_new(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    const FUNC: &str = "stream_bucket_new";
    match &*args[0].deref() {
        Value::Resource(r) if r.kind() == "stream" => {}
        Value::Resource(_) => {
            return Err(Unwind::type_error(format!("{FUNC}(): supplied resource is not a valid stream resource")))
        }
        other => {
            return Err(Unwind::type_error(format!(
                "{FUNC}(): Argument #1 ($stream) must be of type resource, {} given",
                rphp_runtime::value_name(other)
            )))
        }
    }
    let data = str_arg(&args[1], FUNC, 2, "buffer")?;
    bucket_object(ctx, data)
}
