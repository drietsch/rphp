//! `ext/libxml`: the error channel every XML loader in php reports
//! through (`libxml_use_internal_errors()`, `libxml_get_errors()`,
//! `LibXMLError`), the `LIBXML_*` parse options, and the two settings
//! (`libxml_disable_entity_loader()`, `libxml_set_external_entity_loader()`)
//! kept for their return values.

use rphp_runtime::{nf, Ctx, NativeFn, NativeResult, Registry, Unwind};
use rphp_value::{Array, Object, Value};

use crate::parser::XmlError;

/// Per-request state under `ExtState::slots["libxml"]`.
#[derive(Default)]
pub struct LibxmlState {
    pub internal_errors: bool,
    pub errors: Vec<XmlError>,
    pub entity_loader_disabled: bool,
}

pub fn state<'a>(ctx: &'a mut Ctx<'_>) -> &'a mut LibxmlState {
    ctx.ext.slot::<LibxmlState>("libxml")
}

pub const LIBXML_NOENT: i64 = 2;
pub const LIBXML_DTDLOAD: i64 = 4;
pub const LIBXML_DTDATTR: i64 = 8;
pub const LIBXML_DTDVALID: i64 = 16;
pub const LIBXML_NOERROR: i64 = 32;
pub const LIBXML_NOWARNING: i64 = 64;
pub const LIBXML_NOBLANKS: i64 = 256;
pub const LIBXML_XINCLUDE: i64 = 1024;
pub const LIBXML_NONET: i64 = 2048;
pub const LIBXML_NOCDATA: i64 = 16384;
pub const LIBXML_NOXMLDECL: i64 = 2;
pub const LIBXML_COMPACT: i64 = 65536;
pub const LIBXML_PARSEHUGE: i64 = 524288;
pub const LIBXML_BIGLINES: i64 = 4194304;
pub const LIBXML_NOEMPTYTAG: i64 = 4;
pub const LIBXML_HTML_NOIMPLIED: i64 = 8192;
pub const LIBXML_HTML_NODEFDTD: i64 = 4;
pub const LIBXML_SCHEMA_CREATE: i64 = 1;

pub static FUNCTIONS: &[NativeFn] = &[
    nf!(
        "libxml_use_internal_errors",
        0,
        Some(1),
        use_internal_errors
    ),
    nf!("libxml_get_errors", 0, Some(0), get_errors),
    nf!("libxml_get_last_error", 0, Some(0), get_last_error),
    nf!("libxml_clear_errors", 0, Some(0), clear_errors),
    nf!(
        "libxml_disable_entity_loader",
        0,
        Some(1),
        disable_entity_loader
    ),
    nf!(
        "libxml_set_external_entity_loader",
        1,
        Some(1),
        set_external_entity_loader
    ),
    nf!(
        "libxml_get_external_entity_loader",
        0,
        Some(0),
        get_external_entity_loader
    ),
    nf!(
        "libxml_set_streams_context",
        1,
        Some(1),
        set_streams_context
    ),
];

pub fn register_constants(r: &mut Registry) {
    for (name, v) in [
        ("LIBXML_VERSION", 21400),
        ("LIBXML_NOENT", LIBXML_NOENT),
        ("LIBXML_DTDLOAD", LIBXML_DTDLOAD),
        ("LIBXML_DTDATTR", LIBXML_DTDATTR),
        ("LIBXML_DTDVALID", LIBXML_DTDVALID),
        ("LIBXML_NOERROR", LIBXML_NOERROR),
        ("LIBXML_NOWARNING", LIBXML_NOWARNING),
        ("LIBXML_NOBLANKS", LIBXML_NOBLANKS),
        ("LIBXML_XINCLUDE", LIBXML_XINCLUDE),
        ("LIBXML_NONET", LIBXML_NONET),
        ("LIBXML_NOCDATA", LIBXML_NOCDATA),
        ("LIBXML_NOXMLDECL", LIBXML_NOXMLDECL),
        ("LIBXML_COMPACT", LIBXML_COMPACT),
        ("LIBXML_PARSEHUGE", LIBXML_PARSEHUGE),
        ("LIBXML_BIGLINES", LIBXML_BIGLINES),
        ("LIBXML_NOEMPTYTAG", LIBXML_NOEMPTYTAG),
        ("LIBXML_HTML_NOIMPLIED", LIBXML_HTML_NOIMPLIED),
        ("LIBXML_HTML_NODEFDTD", LIBXML_HTML_NODEFDTD),
        ("LIBXML_SCHEMA_CREATE", LIBXML_SCHEMA_CREATE),
        ("LIBXML_ERR_NONE", 0),
        ("LIBXML_ERR_WARNING", 1),
        ("LIBXML_ERR_ERROR", 2),
        ("LIBXML_ERR_FATAL", 3),
        ("LIBXML_NO_XXE", 1 << 23),
        ("LIBXML_RECOVER", 1),
    ] {
        r.constant(name, Value::Int(v));
    }
    r.constant("LIBXML_DOTTED_VERSION", Value::string(b"2.14.0"));
    r.constant("LIBXML_LOADED_VERSION", Value::string(b"21400"));
}

pub fn register_classes(r: &mut Registry) {
    r.class("LibXMLError")
        .prop("level", rphp_runtime::Visibility::Public, Value::Int(0))
        .prop("code", rphp_runtime::Visibility::Public, Value::Int(0))
        .prop("column", rphp_runtime::Visibility::Public, Value::Int(0))
        .prop(
            "message",
            rphp_runtime::Visibility::Public,
            Value::string(b""),
        )
        .prop("file", rphp_runtime::Visibility::Public, Value::string(b""))
        .prop("line", rphp_runtime::Visibility::Public, Value::Int(0))
        .finish();
}

/// A `LibXMLError` object over a diagnostic.
pub fn error_object(ctx: &mut Ctx, e: &XmlError) -> Object {
    let cid = ctx
        .class_by_name(b"LibXMLError")
        .expect("LibXMLError is registered");
    let o = ctx.instantiate(cid);
    o.set(b"level", Value::Int(e.level));
    o.set(b"code", Value::Int(e.code));
    o.set(b"column", Value::Int(e.column));
    o.set(b"message", Value::string(e.message.as_bytes()));
    o.set(
        b"file",
        Value::string(if e.message.ends_with('\n') {
            b""
        } else {
            b"Entity"
        }),
    );
    o.set(b"line", Value::Int(e.line));
    o
}

/// Deliver a parse's diagnostics the way php does: onto the internal
/// list when `libxml_use_internal_errors(true)`, otherwise as php
/// warnings/notices prefixed with the loader's name (`DOMDocument::loadXML():
/// … in Entity, line: N`).
pub fn report(ctx: &mut Ctx, who: &str, errors: Vec<XmlError>, options: i64) -> Result<(), Unwind> {
    if state(ctx).internal_errors {
        state(ctx).errors.extend(errors);
        return Ok(());
    }
    for e in errors {
        if e.level == 1 && options & LIBXML_NOWARNING != 0 {
            continue;
        }
        if e.level >= 2 && options & LIBXML_NOERROR != 0 {
            continue;
        }
        // libxml's messages end in a newline and get their location
        // appended; the HTML5 parser's carry it already.
        let msg = if e.message.ends_with('\n') {
            format!(
                "{who}: {} in Entity, line: {}",
                e.message.trim_end_matches('\n'),
                e.line
            )
        } else {
            format!("{who}: {}", e.message)
        };
        if e.level == 1 {
            ctx.notice(&msg)?;
        } else {
            ctx.warn(&msg)?;
        }
    }
    Ok(())
}

fn use_internal_errors(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let st = state(ctx);
    let old = st.internal_errors;
    if let Some(v) = args.first() {
        if !matches!(v, Value::Null) {
            st.internal_errors = v.to_bool();
            // Switching the buffering off drops what was buffered.
            if !st.internal_errors {
                st.errors.clear();
            }
        }
    }
    Ok(Value::Bool(old))
}

fn get_errors(ctx: &mut Ctx, _: &mut [Value]) -> NativeResult {
    let errors = state(ctx).errors.clone();
    let mut out = Array::new();
    for e in &errors {
        out.push(Value::Object(error_object(ctx, e)));
    }
    Ok(Value::Array(out))
}

fn get_last_error(ctx: &mut Ctx, _: &mut [Value]) -> NativeResult {
    let last = state(ctx).errors.last().cloned();
    Ok(match last {
        Some(e) => Value::Object(error_object(ctx, &e)),
        None => Value::Bool(false),
    })
}

fn clear_errors(ctx: &mut Ctx, _: &mut [Value]) -> NativeResult {
    state(ctx).errors.clear();
    Ok(Value::Null)
}

fn disable_entity_loader(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    ctx.deprecated("Function libxml_disable_entity_loader() is deprecated since 8.0, as external entity loading is disabled by default")?;
    let st = state(ctx);
    let old = st.entity_loader_disabled;
    st.entity_loader_disabled = args.first().is_none_or(Value::to_bool);
    Ok(Value::Bool(old))
}

fn set_external_entity_loader(_: &mut Ctx, _: &mut [Value]) -> NativeResult {
    Ok(Value::Bool(true))
}

fn get_external_entity_loader(_: &mut Ctx, _: &mut [Value]) -> NativeResult {
    Ok(Value::Null)
}

fn set_streams_context(_: &mut Ctx, _: &mut [Value]) -> NativeResult {
    Ok(Value::Null)
}
