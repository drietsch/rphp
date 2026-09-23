//! `ext/xml`: `XMLParser` and the `xml_*` functions — php's expat-style
//! event API as php 8.5 runs it over libxml2 (`XML_SAX_IMPL` is
//! `"libxml"`). The tokenizer is `sax.rs`; this module owns the php
//! surface: the handlers (callables, or method names on the
//! `xml_set_object()` object), case folding, the target encoding,
//! `xml_parse_into_struct()`'s value and index arrays, and the error
//! strings of php's compat layer.

use std::cell::RefCell;
use std::rc::Rc;

use rphp_runtime::{Ctx, NativeFn, NativeResult, Registry, Unwind};
use rphp_value::{Array, ArrayKey, Object, Payload, Value};

use crate::sax::{utf8_at, Ev, Pos, Sax};

/// php's `XML_MAXLEVEL`.
const MAX_LEVEL: i64 = 255;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Target {
    Utf8,
    Latin1,
    Ascii,
}

impl Target {
    fn name(self) -> &'static str {
        match self {
            Target::Utf8 => "UTF-8",
            Target::Latin1 => "ISO-8859-1",
            Target::Ascii => "US-ASCII",
        }
    }

    fn by_name(s: &[u8]) -> Option<Target> {
        [Target::Latin1, Target::Ascii, Target::Utf8]
            .into_iter()
            .find(|t| t.name().as_bytes().eq_ignore_ascii_case(s))
    }
}

/// One `xml_parse_into_struct()` entry.
struct Entry {
    tag: Vec<u8>,
    kind: &'static str,
    level: i64,
    attrs: Option<Vec<(Vec<u8>, Vec<u8>)>>,
    value: Option<Vec<u8>>,
}

#[derive(Default)]
struct Handlers {
    start: Option<Value>,
    end: Option<Value>,
    cdata: Option<Value>,
    pi: Option<Value>,
    default: Option<Value>,
    unparsed: Option<Value>,
    notation: Option<Value>,
    extref: Option<Value>,
    ns_start: Option<Value>,
    ns_end: Option<Value>,
}

pub struct Parser {
    sax: Sax,
    target: Target,
    case_folding: bool,
    skip_tagstart: usize,
    skip_white: bool,
    parse_huge: bool,
    object: Option<Object>,
    h: Handlers,
    parsing: bool,
    level: i64,
    ltags: Vec<Vec<u8>>,
    last_was_open: bool,
    ctag: Option<usize>,
    data: Option<Vec<Entry>>,
    info: Option<Vec<(Vec<u8>, Vec<i64>)>>,
    /// The position a running handler sees.
    at: Option<Pos>,
}

type Shared = Rc<RefCell<Parser>>;

pub static FUNCTIONS: &[NativeFn] = &[
    f("xml_parser_create", 0, 1, 0, &["encoding"], xml_parser_create),
    f("xml_parser_create_ns", 0, 2, 0, &["encoding", "separator"], xml_parser_create_ns),
    f("xml_set_object", 2, 2, 0, &["parser", "object"], xml_set_object),
    f("xml_set_element_handler", 3, 3, 0, &["parser", "start_handler", "end_handler"], xml_set_element_handler),
    f("xml_set_character_data_handler", 2, 2, 0, &["parser", "handler"], xml_set_character_data_handler),
    f("xml_set_processing_instruction_handler", 2, 2, 0, &["parser", "handler"], xml_set_pi_handler),
    f("xml_set_default_handler", 2, 2, 0, &["parser", "handler"], xml_set_default_handler),
    f("xml_set_unparsed_entity_decl_handler", 2, 2, 0, &["parser", "handler"], xml_set_unparsed_handler),
    f("xml_set_notation_decl_handler", 2, 2, 0, &["parser", "handler"], xml_set_notation_handler),
    f("xml_set_external_entity_ref_handler", 2, 2, 0, &["parser", "handler"], xml_set_extref_handler),
    f("xml_set_start_namespace_decl_handler", 2, 2, 0, &["parser", "handler"], xml_set_ns_start_handler),
    f("xml_set_end_namespace_decl_handler", 2, 2, 0, &["parser", "handler"], xml_set_ns_end_handler),
    f("xml_parse", 2, 3, 0, &["parser", "data", "is_final"], xml_parse),
    f("xml_parse_into_struct", 3, 4, 0b1100, &["parser", "data", "values", "index"], xml_parse_into_struct),
    f("xml_get_error_code", 1, 1, 0, &["parser"], xml_get_error_code),
    f("xml_error_string", 1, 1, 0, &["error_code"], xml_error_string),
    f("xml_get_current_line_number", 1, 1, 0, &["parser"], xml_get_current_line_number),
    f("xml_get_current_column_number", 1, 1, 0, &["parser"], xml_get_current_column_number),
    f("xml_get_current_byte_index", 1, 1, 0, &["parser"], xml_get_current_byte_index),
    f("xml_parser_free", 1, 1, 0, &["parser"], xml_parser_free),
    f("xml_parser_set_option", 3, 3, 0, &["parser", "option", "value"], xml_parser_set_option),
    f("xml_parser_get_option", 2, 2, 0, &["parser", "option"], xml_parser_get_option),
];

const fn f(
    name: &'static str,
    min: u8,
    max: u8,
    by_ref: u32,
    params: &'static [&'static str],
    handler: rphp_runtime::NativeHandler,
) -> NativeFn {
    NativeFn {
        name,
        min_args: min,
        max_args: Some(max),
        by_ref,
        params,
        flags: rphp_runtime::FnFlags::EMPTY,
        handler,
    }
}

pub fn register(r: &mut Registry) {
    r.class("XMLParser")
        .flags(rphp_runtime::ClassFlags::FINAL)
        .native_init(not_constructible)
        .uncloneable()
        .finish();
    r.functions(FUNCTIONS);
    for (i, name) in [
        "XML_ERROR_NONE",
        "XML_ERROR_NO_MEMORY",
        "XML_ERROR_SYNTAX",
        "XML_ERROR_NO_ELEMENTS",
        "XML_ERROR_INVALID_TOKEN",
        "XML_ERROR_UNCLOSED_TOKEN",
        "XML_ERROR_PARTIAL_CHAR",
        "XML_ERROR_TAG_MISMATCH",
        "XML_ERROR_DUPLICATE_ATTRIBUTE",
        "XML_ERROR_JUNK_AFTER_DOC_ELEMENT",
        "XML_ERROR_PARAM_ENTITY_REF",
        "XML_ERROR_UNDEFINED_ENTITY",
        "XML_ERROR_RECURSIVE_ENTITY_REF",
        "XML_ERROR_ASYNC_ENTITY",
        "XML_ERROR_BAD_CHAR_REF",
        "XML_ERROR_BINARY_ENTITY_REF",
        "XML_ERROR_ATTRIBUTE_EXTERNAL_ENTITY_REF",
        "XML_ERROR_MISPLACED_XML_PI",
        "XML_ERROR_UNKNOWN_ENCODING",
        "XML_ERROR_INCORRECT_ENCODING",
        "XML_ERROR_UNCLOSED_CDATA_SECTION",
        "XML_ERROR_EXTERNAL_ENTITY_HANDLING",
    ]
    .into_iter()
    .enumerate()
    {
        r.constant(name, Value::Int(i as i64));
    }
    for (name, v) in [
        ("XML_OPTION_CASE_FOLDING", OPT_CASE_FOLDING),
        ("XML_OPTION_TARGET_ENCODING", OPT_TARGET_ENCODING),
        ("XML_OPTION_SKIP_TAGSTART", OPT_SKIP_TAGSTART),
        ("XML_OPTION_SKIP_WHITE", OPT_SKIP_WHITE),
        ("XML_OPTION_PARSE_HUGE", OPT_PARSE_HUGE),
    ] {
        r.constant(name, Value::Int(v));
    }
    r.constant("XML_SAX_IMPL", Value::string(b"libxml"));
}

const OPT_CASE_FOLDING: i64 = 1;
const OPT_TARGET_ENCODING: i64 = 2;
const OPT_SKIP_TAGSTART: i64 = 3;
const OPT_SKIP_WHITE: i64 = 4;
const OPT_PARSE_HUGE: i64 = 5;

/// `new XMLParser` is refused (php's `get_constructor` handler); the
/// hook runs on `new` only, not for the parsers `xml_parser_create()` makes.
fn not_constructible(_: &mut rphp_runtime::Interp, _: &Object) -> Result<(), Unwind> {
    Err(Unwind::error(
        "Cannot directly construct XMLParser, use xml_parser_create() or xml_parser_create_ns() instead",
    ))
}

// ---- arguments ---------------------------------------------------------

fn bytes(args: &[Value], i: usize) -> Vec<u8> {
    args.get(i).map(|v| v.deref().to_php_bytes()).unwrap_or_default()
}

fn parser_arg(ctx: &mut Ctx, args: &[Value]) -> Result<(Object, Shared), Unwind> {
    let v = args.first().map(|v| v.deref().into_owned()).unwrap_or(Value::Null);
    if let Value::Object(o) = &v {
        if let Some(p) = o.with_payload::<Shared, _>(|p| p.clone()) {
            return Ok((o.clone(), p));
        }
    }
    Err(Unwind::type_error(format!(
        "{}(): Argument #1 ($parser) must be of type XMLParser, {} given",
        ctx.active_function_name(),
        rphp_runtime::value_name(&v)
    )))
}

fn create(ctx: &mut Ctx, args: &[Value], ns: bool) -> NativeResult {
    let who = ctx.active_function_name();
    let target = match args.first().map(|v| v.deref().into_owned()) {
        None | Some(Value::Null) => Target::Utf8,
        Some(v) => {
            let s = v.to_php_bytes();
            if s.is_empty() {
                Target::Utf8
            } else {
                Target::by_name(&s).ok_or_else(|| {
                    Unwind::value_error(format!(
                        "{who}(): Argument #1 ($encoding) is not a supported source encoding"
                    ))
                })?
            }
        }
    };
    let sep = if ns {
        args.get(1).map(|v| v.deref().to_php_bytes()).unwrap_or_else(|| b":".to_vec())
    } else {
        Vec::new()
    };
    let parser = Parser {
        sax: Sax::new(ns, &sep),
        target,
        case_folding: true,
        skip_tagstart: 0,
        skip_white: false,
        parse_huge: false,
        object: None,
        h: Handlers::default(),
        parsing: false,
        level: 0,
        ltags: Vec::new(),
        last_was_open: false,
        ctag: None,
        data: None,
        info: None,
        at: None,
    };
    let cid = ctx.lookup_class_or_error(b"XMLParser")?;
    let o = ctx.instantiate(cid);
    o.set_payload(Payload::Native(Box::new(Rc::new(RefCell::new(parser)))));
    Ok(Value::Object(o))
}

fn xml_parser_create(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    create(ctx, args, false)
}

fn xml_parser_create_ns(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    create(ctx, args, true)
}

fn xml_parser_free(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    ctx.deprecated("Function xml_parser_free() is deprecated since 8.5, as it has no effect since PHP 8.0")?;
    let (_, p) = parser_arg(ctx, args)?;
    if p.borrow().parsing {
        ctx.warn("xml_parser_free(): Parser cannot be freed while it is parsing")?;
        return Ok(Value::Bool(false));
    }
    Ok(Value::Bool(true))
}

fn xml_set_object(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    ctx.deprecated(
        "Function xml_set_object() is deprecated since 8.4, provide a proper method callable to xml_set_*_handler() functions",
    )?;
    let (_, p) = parser_arg(ctx, args)?;
    let obj = match args.get(1).map(|v| v.deref().into_owned()) {
        Some(Value::Object(o)) => o,
        other => {
            return Err(Unwind::type_error(format!(
                "xml_set_object(): Argument #2 ($object) must be of type object, {} given",
                rphp_runtime::value_name(&other.unwrap_or(Value::Null))
            )))
        }
    };
    p.borrow_mut().object = Some(obj);
    Ok(Value::Bool(true))
}

/// A handler argument as php's parameter parsing sorts it.
enum HArg {
    Unset,
    Callable(Value),
    /// A non-callable string: a method name on the `xml_set_object()`
    /// object (deprecated since 8.4).
    Method(Vec<u8>),
}

fn classify(ctx: &mut Ctx, v: &Value, argn: usize, pname: &str) -> Result<HArg, Unwind> {
    let v = v.deref().into_owned();
    if matches!(v, Value::Null) {
        return Ok(HArg::Unset);
    }
    if ctx.is_callable(&v) {
        return Ok(HArg::Callable(v));
    }
    if matches!(v, Value::Str(_) | Value::Int(_) | Value::Float(_) | Value::Bool(_)) {
        return Ok(HArg::Method(v.to_php_bytes()));
    }
    Err(Unwind::type_error(format!(
        "{}(): Argument #{argn} (${pname}) must be of type callable|string|null",
        ctx.active_function_name()
    )))
}

fn resolve(ctx: &mut Ctx, p: &Shared, h: HArg, argn: usize, pname: &str) -> Result<Option<Value>, Unwind> {
    let name = match h {
        HArg::Unset => return Ok(None),
        HArg::Callable(v) => return Ok(Some(v)),
        HArg::Method(name) => name,
    };
    if name.is_empty() {
        return Ok(None);
    }
    let who = ctx.active_function_name();
    let Some(obj) = p.borrow().object.clone() else {
        return Err(Unwind::value_error(format!(
            "{who}(): Argument #{argn} (${pname}) an object must be set via xml_set_object() to be able to lookup method"
        )));
    };
    if ctx.resolve_method(obj.class_id(), &name).is_none() {
        return Err(Unwind::value_error(format!(
            "{who}(): Argument #{argn} (${pname}) method {}::{}() does not exist",
            ctx.class_name_of(&obj),
            String::from_utf8_lossy(&name)
        )));
    }
    let mut a = Array::new();
    a.push(Value::Object(obj));
    a.push(Value::string(&name));
    Ok(Some(Value::Array(a)))
}

fn deprecate_strings(ctx: &mut Ctx) -> Result<(), Unwind> {
    let who = ctx.active_function_name();
    ctx.deprecated(&format!("{who}(): Passing non-callable strings is deprecated since 8.4"))
}

fn set_one(ctx: &mut Ctx, args: &[Value], slot: fn(&mut Handlers) -> &mut Option<Value>) -> NativeResult {
    let (_, p) = parser_arg(ctx, args)?;
    let h = classify(ctx, &args[1], 2, "handler")?;
    if matches!(h, HArg::Method(_)) {
        deprecate_strings(ctx)?;
    }
    let h = resolve(ctx, &p, h, 2, "handler")?;
    *slot(&mut p.borrow_mut().h) = h;
    Ok(Value::Bool(true))
}

fn xml_set_element_handler(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let (_, p) = parser_arg(ctx, args)?;
    let s = classify(ctx, &args[1], 2, "start_handler")?;
    let e = classify(ctx, &args[2], 3, "end_handler")?;
    if matches!(s, HArg::Method(_)) || matches!(e, HArg::Method(_)) {
        deprecate_strings(ctx)?;
    }
    let s = resolve(ctx, &p, s, 2, "start_handler")?;
    let e = resolve(ctx, &p, e, 3, "end_handler")?;
    let mut b = p.borrow_mut();
    b.h.start = s;
    b.h.end = e;
    Ok(Value::Bool(true))
}

fn xml_set_character_data_handler(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    set_one(ctx, args, |h| &mut h.cdata)
}

fn xml_set_pi_handler(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    set_one(ctx, args, |h| &mut h.pi)
}

fn xml_set_default_handler(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    set_one(ctx, args, |h| &mut h.default)
}

fn xml_set_unparsed_handler(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    set_one(ctx, args, |h| &mut h.unparsed)
}

fn xml_set_notation_handler(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    set_one(ctx, args, |h| &mut h.notation)
}

fn xml_set_extref_handler(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    set_one(ctx, args, |h| &mut h.extref)
}

fn xml_set_ns_start_handler(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    set_one(ctx, args, |h| &mut h.ns_start)
}

fn xml_set_ns_end_handler(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    set_one(ctx, args, |h| &mut h.ns_end)
}

// ---- parsing -------------------------------------------------------------

fn xml_parse(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let (o, p) = parser_arg(ctx, args)?;
    let data = bytes(args, 1);
    let fin = args.get(2).is_some_and(|v| v.deref().to_bool());
    if p.borrow().parsing {
        return Err(Unwind::error("Parser must not be called recursively"));
    }
    let ok = run(ctx, &o, &p, &data, fin)?;
    Ok(Value::Int(ok as i64))
}

fn xml_parse_into_struct(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let (o, p) = parser_arg(ctx, args)?;
    let data = bytes(args, 1);
    if p.borrow().parsing {
        return Err(Unwind::error("Parser must not be called recursively"));
    }
    let with_index = args.len() > 3;
    {
        let mut b = p.borrow_mut();
        b.data = Some(Vec::new());
        b.info = with_index.then(Vec::new);
        b.level = 0;
        b.ltags.clear();
    }
    let res = run(ctx, &o, &p, &data, true);
    let (entries, info) = {
        let mut b = p.borrow_mut();
        (b.data.take().unwrap_or_default(), b.info.take())
    };
    let ok = res?;
    let mut values = Array::new();
    for e in entries {
        let mut a = Array::new();
        a.set(ArrayKey::str(b"tag"), Value::string(&e.tag));
        if e.kind == "cdata" {
            a.set(ArrayKey::str(b"value"), Value::string(e.value.as_deref().unwrap_or(b"")));
            a.set(ArrayKey::str(b"type"), Value::string(e.kind.as_bytes()));
            a.set(ArrayKey::str(b"level"), Value::Int(e.level));
        } else {
            a.set(ArrayKey::str(b"type"), Value::string(e.kind.as_bytes()));
            a.set(ArrayKey::str(b"level"), Value::Int(e.level));
            if let Some(attrs) = &e.attrs {
                a.set(ArrayKey::str(b"attributes"), Value::Array(attr_array(attrs)));
            }
            if let Some(v) = &e.value {
                a.set(ArrayKey::str(b"value"), Value::string(v));
            }
        }
        values.push(Value::Array(a));
    }
    args[2] = Value::Array(values);
    if let Some(info) = info {
        let mut idx = Array::new();
        for (name, list) in info {
            let mut l = Array::new();
            for i in list {
                l.push(Value::Int(i));
            }
            idx.set(ArrayKey::str(&name), Value::Array(l));
        }
        args[3] = Value::Array(idx);
    }
    Ok(Value::Int(ok as i64))
}

fn attr_array(attrs: &[(Vec<u8>, Vec<u8>)]) -> Array {
    let mut a = Array::new();
    for (k, v) in attrs {
        a.set(symtable_key(k), Value::string(v));
    }
    a
}

/// `zend_symtable_update`: a canonical decimal string is an integer key.
fn symtable_key(k: &[u8]) -> ArrayKey {
    rphp_value::array_key(&Value::string(k)).unwrap_or_else(|| ArrayKey::str(k))
}

/// Feed the tokenizer and deliver its events to the handlers.
fn run(ctx: &mut Ctx, o: &Object, p: &Shared, data: &[u8], fin: bool) -> Result<bool, Unwind> {
    let (ok, events) = {
        let mut b = p.borrow_mut();
        b.parsing = true;
        let ok = b.sax.feed(data, fin);
        (ok, std::mem::take(&mut b.sax.events))
    };
    let mut res = Ok(());
    for (ev, pos) in events {
        p.borrow_mut().at = Some(pos);
        if let Err(e) = dispatch(ctx, o, p, ev) {
            res = Err(e);
            break;
        }
    }
    {
        let mut b = p.borrow_mut();
        b.at = None;
        b.parsing = false;
    }
    res?;
    Ok(ok)
}

impl Parser {
    /// `xml_utf8_decode`: UTF-8 into the target encoding (a character
    /// the target cannot hold becomes `?`).
    fn decode(&self, s: &[u8]) -> Vec<u8> {
        if self.target == Target::Utf8 {
            return s.to_vec();
        }
        let max = if self.target == Target::Latin1 { 0xFF } else { 0x7F };
        let mut out = Vec::with_capacity(s.len());
        let mut i = 0;
        while i < s.len() {
            match utf8_at(&s[i..]) {
                Some((c, l)) => {
                    out.push(if c > max { b'?' } else { c as u8 });
                    i += l;
                }
                None => {
                    out.push(b'?');
                    i += 1;
                }
            }
        }
        out
    }

    /// `_xml_decode_tag`: decoded, upper-cased under case folding.
    fn decode_tag(&self, s: &[u8]) -> Vec<u8> {
        let mut t = self.decode(s);
        if self.case_folding {
            t.make_ascii_uppercase();
        }
        t
    }

    fn skipped(&self, t: &[u8]) -> Vec<u8> {
        t[self.skip_tagstart.min(t.len())..].to_vec()
    }

    fn add_to_info(&mut self, name: &[u8]) {
        let n = self.data.as_ref().map_or(0, Vec::len) as i64;
        if let Some(info) = &mut self.info {
            match info.iter_mut().find(|(k, _)| k.as_slice() == name) {
                Some((_, l)) => l.push(n),
                None => info.push((name.to_vec(), vec![n])),
            }
        }
    }
}

fn call(ctx: &mut Ctx, h: Option<Value>, args: &[Value]) -> Result<(), Unwind> {
    if let Some(h) = h {
        ctx.call_value(&h, args)?;
    }
    Ok(())
}

fn opt_str(p: &Parser, v: Option<&[u8]>) -> Value {
    match v {
        Some(s) => Value::string(&p.decode(s)),
        None => Value::Bool(false),
    }
}

fn dispatch(ctx: &mut Ctx, o: &Object, p: &Shared, ev: Ev) -> Result<(), Unwind> {
    let me = Value::Object(o.clone());
    match ev {
        Ev::Start { name, attrs, ns } => {
            for (prefix, uri) in ns {
                let (h, a) = {
                    let b = p.borrow();
                    (b.h.ns_start.clone(), [me.clone(), opt_str(&b, prefix.as_deref()), Value::string(&b.decode(&uri))])
                };
                call(ctx, h, &a)?;
            }
            let (h, tag, attrs) = {
                let mut b = p.borrow_mut();
                b.level += 1;
                let tag = b.decode_tag(&name);
                let attrs: Vec<(Vec<u8>, Vec<u8>)> =
                    attrs.iter().map(|(k, v)| (b.decode_tag(k), b.decode(v))).collect();
                (b.h.start.clone(), tag, attrs)
            };
            if h.is_some() {
                let skipped = p.borrow().skipped(&tag);
                call(ctx, h, &[me, Value::string(&skipped), Value::Array(attr_array(&attrs))])?;
            }
            let mut b = p.borrow_mut();
            if b.data.is_some() {
                if b.level <= MAX_LEVEL {
                    let skipped = b.skipped(&tag);
                    b.add_to_info(&skipped);
                    let level = b.level;
                    let lv = level as usize;
                    if b.ltags.len() < lv {
                        b.ltags.resize(lv, Vec::new());
                    }
                    b.ltags[lv - 1] = tag;
                    b.last_was_open = true;
                    let data = b.data.as_mut().expect("data");
                    data.push(Entry {
                        tag: skipped,
                        kind: "open",
                        level,
                        attrs: (!attrs.is_empty()).then_some(attrs),
                        value: None,
                    });
                    let n = data.len() - 1;
                    b.ctag = Some(n);
                } else if b.level == MAX_LEVEL + 1 {
                    drop(b);
                    ctx.warn("xml_parse(): Maximum depth exceeded - Results truncated")?;
                }
            }
        }
        Ev::End(name) => {
            let (h, tag) = {
                let b = p.borrow();
                (b.h.end.clone(), b.decode_tag(&name))
            };
            if h.is_some() {
                let skipped = p.borrow().skipped(&tag);
                call(ctx, h, &[me, Value::string(&skipped)])?;
            }
            let mut b = p.borrow_mut();
            if b.data.is_some() {
                if b.last_was_open {
                    if let Some(i) = b.ctag {
                        b.data.as_mut().expect("data")[i].kind = "complete";
                    }
                } else {
                    let skipped = b.skipped(&tag);
                    b.add_to_info(&skipped);
                    let level = b.level;
                    b.data.as_mut().expect("data").push(Entry {
                        tag: skipped,
                        kind: "close",
                        level,
                        attrs: None,
                        value: None,
                    });
                }
                b.last_was_open = false;
            }
            b.level -= 1;
        }
        Ev::Chars(s) => character_data(ctx, &me, p, &s)?,
        Ev::Pi(target, data) => {
            let b = p.borrow();
            if b.h.pi.is_some() {
                let a = [me, Value::string(&b.decode(&target)), opt_str(&b, data.as_deref())];
                let h = b.h.pi.clone();
                drop(b);
                call(ctx, h, &a)?;
            } else if b.h.default.is_some() {
                let mut text = b"<?".to_vec();
                text.extend_from_slice(&target);
                text.push(b' ');
                text.extend_from_slice(data.as_deref().unwrap_or(b"(null)"));
                text.extend_from_slice(b"?>");
                let a = [me, Value::string(&b.decode(&text))];
                let h = b.h.default.clone();
                drop(b);
                call(ctx, h, &a)?;
            }
        }
        Ev::Comment(c) => {
            let b = p.borrow();
            if b.h.default.is_some() {
                let text = [b"<!--".as_slice(), &c, b"-->"].concat();
                let a = [me, Value::string(&b.decode(&text))];
                let h = b.h.default.clone();
                drop(b);
                call(ctx, h, &a)?;
            }
        }
        Ev::EntityRef(name, value) => {
            let b = p.borrow();
            if b.h.default.is_some() {
                let text = [b"&".as_slice(), &name, b";"].concat();
                let a = [me, Value::string(&b.decode(&text))];
                let h = b.h.default.clone();
                drop(b);
                call(ctx, h, &a)?;
            } else if let Some(v) = value {
                drop(b);
                character_data(ctx, &me, p, &v)?;
            }
        }
        Ev::ExternalRef(name, system, public) => {
            let b = p.borrow();
            if b.h.extref.is_some() {
                let a = [
                    me,
                    Value::string(&b.decode(&name)),
                    Value::string(b""),
                    Value::string(&b.decode(&system)),
                    opt_str(&b, public.as_deref()),
                ];
                let h = b.h.extref.clone();
                drop(b);
                call(ctx, h, &a)?;
            }
        }
        Ev::Unparsed(name, system, public, notation) => {
            let b = p.borrow();
            if b.h.unparsed.is_some() {
                let a = [
                    me,
                    Value::string(&b.decode(&name)),
                    Value::Bool(false),
                    Value::string(&b.decode(&system)),
                    opt_str(&b, public.as_deref()),
                    Value::string(&b.decode(&notation)),
                ];
                let h = b.h.unparsed.clone();
                drop(b);
                call(ctx, h, &a)?;
            }
        }
        Ev::Notation(name, system, public) => {
            let b = p.borrow();
            if b.h.notation.is_some() {
                let a = [
                    me,
                    Value::string(&b.decode(&name)),
                    Value::Bool(false),
                    opt_str(&b, system.as_deref()),
                    opt_str(&b, public.as_deref()),
                ];
                let h = b.h.notation.clone();
                drop(b);
                call(ctx, h, &a)?;
            }
        }
    }
    Ok(())
}

/// `_xml_characterDataHandler`: the handler, then the struct's value.
fn character_data(ctx: &mut Ctx, me: &Value, p: &Shared, s: &[u8]) -> Result<(), Unwind> {
    let (h, decoded) = {
        let b = p.borrow();
        (b.h.cdata.clone(), b.decode(s))
    };
    if h.is_some() {
        call(ctx, h, &[me.clone(), Value::string(&decoded)])?;
    }
    let mut b = p.borrow_mut();
    if b.data.is_none() {
        return Ok(());
    }
    let doprint = !b.skip_white || decoded.iter().any(|c| !matches!(c, b' ' | b'\t' | b'\n'));
    if b.last_was_open {
        let Some(i) = b.ctag else { return Ok(()) };
        let skip_white = b.skip_white;
        let e = &mut b.data.as_mut().expect("data")[i];
        match &mut e.value {
            Some(v) => v.extend_from_slice(&decoded),
            None => {
                if doprint || !skip_white {
                    e.value = Some(decoded);
                }
            }
        }
        return Ok(());
    }
    if let Some(last) = b.data.as_mut().expect("data").last_mut() {
        if last.kind == "cdata" {
            if let Some(v) = &mut last.value {
                v.extend_from_slice(&decoded);
                return Ok(());
            }
        }
    } else {
        return Ok(());
    }
    let level = b.level;
    if level <= MAX_LEVEL && level > 0 && doprint {
        let tag = b.ltags.get(level as usize - 1).cloned().unwrap_or_default();
        let skipped = b.skipped(&tag);
        b.add_to_info(&skipped);
        b.data.as_mut().expect("data").push(Entry {
            tag: skipped,
            kind: "cdata",
            level,
            attrs: None,
            value: Some(decoded),
        });
    } else if level == MAX_LEVEL + 1 {
        drop(b);
        ctx.warn("xml_parse(): Maximum depth exceeded - Results truncated")?;
    }
    Ok(())
}

// ---- state queries -----------------------------------------------------

fn pos_of(p: &Shared) -> Pos {
    let b = p.borrow();
    b.at.unwrap_or_else(|| b.sax.pos())
}

fn xml_get_error_code(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let (_, p) = parser_arg(ctx, args)?;
    let e = p.borrow().sax.err;
    Ok(Value::Int(e))
}

fn xml_get_current_line_number(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let (_, p) = parser_arg(ctx, args)?;
    Ok(Value::Int(pos_of(&p).line))
}

fn xml_get_current_column_number(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let (_, p) = parser_arg(ctx, args)?;
    Ok(Value::Int(pos_of(&p).col))
}

fn xml_get_current_byte_index(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let (_, p) = parser_arg(ctx, args)?;
    Ok(Value::Int(pos_of(&p).byte))
}

fn xml_error_string(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let code = args[0].deref().to_int();
    let s = usize::try_from(code).ok().and_then(|c| ERRORS.get(c)).copied().unwrap_or("Unknown");
    Ok(Value::string(s.as_bytes()))
}

fn option_arg(ctx: &Ctx, args: &[Value]) -> Result<i64, Unwind> {
    let opt = args[1].deref().to_int();
    if !(1..=5).contains(&opt) {
        return Err(Unwind::value_error(format!(
            "{}(): Argument #2 ($option) must be a XML_OPTION_* constant",
            ctx.active_function_name()
        )));
    }
    Ok(opt)
}

fn xml_parser_set_option(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let (_, p) = parser_arg(ctx, args)?;
    let value = args[2].deref().into_owned();
    if !matches!(value, Value::Bool(_) | Value::Int(_) | Value::Str(_)) {
        let t = match &value {
            Value::Null => "null",
            Value::Float(_) => "float",
            Value::Array(_) => "array",
            _ => "object",
        };
        ctx.warn(&format!(
            "xml_parser_set_option(): Argument #3 ($value) must be of type string|int|bool, {t} given"
        ))?;
    }
    let opt = option_arg(ctx, args)?;
    match opt {
        OPT_CASE_FOLDING => p.borrow_mut().case_folding = value.to_bool(),
        OPT_SKIP_WHITE => p.borrow_mut().skip_white = value.to_bool(),
        OPT_PARSE_HUGE => {
            if p.borrow().parsing {
                return Err(Unwind::error("Cannot change option XML_OPTION_PARSE_HUGE while parsing"));
            }
            p.borrow_mut().parse_huge = value.to_bool();
        }
        OPT_SKIP_TAGSTART => {
            let n = value.to_int();
            if !(0..=i32::MAX as i64).contains(&n) {
                ctx.warn("xml_parser_set_option(): Argument #3 ($value) must be between 0 and 2147483647 for option XML_OPTION_SKIP_TAGSTART")?;
                return Ok(Value::Bool(false));
            }
            p.borrow_mut().skip_tagstart = n as usize;
        }
        _ => {
            let s = value.to_php_bytes();
            let Some(t) = Target::by_name(&s) else {
                return Err(Unwind::value_error(
                    "xml_parser_set_option(): Argument #3 ($value) is not a supported target encoding",
                ));
            };
            p.borrow_mut().target = t;
        }
    }
    Ok(Value::Bool(true))
}

fn xml_parser_get_option(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let (_, p) = parser_arg(ctx, args)?;
    let opt = option_arg(ctx, args)?;
    let b = p.borrow();
    Ok(match opt {
        OPT_CASE_FOLDING => Value::Bool(b.case_folding),
        OPT_SKIP_WHITE => Value::Bool(b.skip_white),
        OPT_PARSE_HUGE => Value::Bool(b.parse_huge),
        OPT_SKIP_TAGSTART => Value::Int(b.skip_tagstart as i64),
        _ => Value::string(b.target.name().as_bytes()),
    })
}

/// php's compat layer's `error_mapping`, indexed by libxml2 error number.
static ERRORS: &[&str] = &[
    "No error",
    "No memory",
    "Invalid document start",
    "Empty document",
    "Not well-formed (invalid token)",
    "Invalid document end",
    "Invalid hexadecimal character reference",
    "Invalid decimal character reference",
    "Invalid character reference",
    "Invalid character",
    "XML_ERR_CHARREF_AT_EOF",
    "XML_ERR_CHARREF_IN_PROLOG",
    "XML_ERR_CHARREF_IN_EPILOG",
    "XML_ERR_CHARREF_IN_DTD",
    "XML_ERR_ENTITYREF_AT_EOF",
    "XML_ERR_ENTITYREF_IN_PROLOG",
    "XML_ERR_ENTITYREF_IN_EPILOG",
    "XML_ERR_ENTITYREF_IN_DTD",
    "PEReference at end of document",
    "PEReference in prolog",
    "PEReference in epilog",
    "PEReference: forbidden within markup decl in internal subset",
    "XML_ERR_ENTITYREF_NO_NAME",
    "EntityRef: expecting ';'",
    "PEReference: no name",
    "PEReference: expecting ';'",
    "Undeclared entity error",
    "Undeclared entity warning",
    "Unparsed Entity",
    "XML_ERR_ENTITY_IS_EXTERNAL",
    "XML_ERR_ENTITY_IS_PARAMETER",
    "Unknown encoding",
    "Unsupported encoding",
    "String not started expecting ' or \"",
    "String not closed expecting \" or '",
    "Namespace declaration error",
    "EntityValue: \" or ' expected",
    "EntityValue: \" or ' expected",
    "< in attribute",
    "Attribute not started",
    "Attribute not finished",
    "Attribute without value",
    "Attribute redefined",
    "SystemLiteral \" or ' expected",
    "SystemLiteral \" or ' expected",
    "Comment not finished",
    "Processing Instruction not started",
    "Processing Instruction not finished",
    "NOTATION: Name expected here",
    "'>' required to close NOTATION declaration",
    "'(' required to start ATTLIST enumeration",
    "'(' required to start ATTLIST enumeration",
    "MixedContentDecl : '|' or ')*' expected",
    "XML_ERR_MIXED_NOT_FINISHED",
    "ELEMENT in DTD not started",
    "ELEMENT in DTD not finished",
    "XML declaration not started",
    "XML declaration not finished",
    "XML_ERR_CONDSEC_NOT_STARTED",
    "XML conditional section not closed",
    "Content error in the external subset",
    "DOCTYPE not finished",
    "Sequence ']]>' not allowed in content",
    "CDATA not finished",
    "Reserved XML Name",
    "Space required",
    "XML_ERR_SEPARATOR_REQUIRED",
    "NmToken expected in ATTLIST enumeration",
    "XML_ERR_NAME_REQUIRED",
    "MixedContentDecl : '#PCDATA' expected",
    "SYSTEM or PUBLIC, the URI is missing",
    "PUBLIC, the Public Identifier is missing",
    "< required",
    "> required",
    "</ required",
    "= required",
    "Mismatched tag",
    "Tag not finished",
    "standalone accepts only 'yes' or 'no'",
    "Invalid XML encoding name",
    "Comment must not contain '--' (double-hyphen)",
    "Invalid encoding",
    "external parsed entities cannot be standalone",
    "XML conditional section '[' expected",
    "Entity value required",
    "chunk is not well balanced",
    "extra content at the end of well balanced chunk",
    "XML_ERR_ENTITY_CHAR_ERROR",
    "PEReferences forbidden in internal subset",
    "Detected an entity reference loop",
    "XML_ERR_ENTITY_BOUNDARY",
    "Invalid URI",
    "Fragment not allowed",
    "XML_WAR_CATALOG_PI",
    "XML_ERR_NO_DTD",
    "conditional section INCLUDE or IGNORE keyword expected",
    "Version in XML Declaration missing",
    "XML_WAR_UNKNOWN_VERSION",
    "XML_WAR_LANG_VALUE",
    "XML_WAR_NS_URI",
    "XML_WAR_NS_URI_RELATIVE",
    "Missing encoding in text declaration",
];
