//! Output / debugging builtins: `var_dump`, `print_r`.
//!
//! These write to the run's stdout buffer ([`Ctx::out`]) rather than returning a
//! string (except `print_r($v, true)`, which returns it). The formatting is
//! byte-exact against stock PHP for scalars, arrays, objects (class name,
//! handle id, `:protected` / `:"Decl":private` annotations, `*RECURSION*`),
//! references (`&` on a shared element) and resources. Floats use
//! [`php_gcvt`], the `serialize_precision` form shared with
//! `var_export`/`serialize` (`var.rs`). Closures keep a placeholder shape
//! until they become `Closure` objects (plan E6).
use rphp_value::{display_class_name, ArrayKey, Object, PhpRef, Str, Value, Vis};

use rphp_runtime::{nf, Ctx, MagicFlags, NativeFn, NativeResult, Unwind};

/// This extension's registry contribution (see `lib.rs`).
pub(crate) static FUNCTIONS: &[NativeFn] = &[
    nf!("var_dump", 1, None, var_dump),
    nf!("print_r", 1, Some(2), print_r),
];

/// Containers currently being printed, to detect cycles: object ids
/// (`$o->self = $o`) and reference cells (`$a[0] = &$a`). php protects the
/// array itself; a cell shared with an ancestor is the closest the safe heap
/// can observe, so a self-referential array prints one nesting level before
/// `*RECURSION*` where php prints none.
#[derive(Default)]
struct Seen {
    objects: Vec<u32>,
    refs: Vec<PhpRef>,
}

impl Seen {
    fn new() -> Seen {
        Seen::default()
    }

    /// Whether `r` is on the print path; pushes it otherwise.
    fn enter_ref(&mut self, r: &PhpRef) -> bool {
        if self.refs.iter().any(|s| s.ptr_eq(r)) {
            return false;
        }
        self.refs.push(r.clone());
        true
    }
}

/// PHP `var_dump(...$values)`: dump each argument's type and value. Returns null.
pub(crate) fn var_dump(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let mut seen = Seen::new();
    let precision = serialize_precision(ctx);
    let mut buf = Vec::new();
    {
        let mut native = |v: &Value| native_dump_props(ctx, v);
        for v in args.iter() {
            dump(&mut buf, v, 0, &mut seen, precision, &mut native)?;
        }
    }
    ctx.out().extend_from_slice(&buf);
    Ok(Value::Null)
}

/// PHP `print_r($value, $return = false)`: human-readable form. With `$return`
/// truthy, returns the string; otherwise writes it to stdout and returns `true`.
pub(crate) fn print_r(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let return_mode = args.get(1).is_some_and(Value::to_bool);
    let mut buf = Vec::new();
    {
        let mut native = |v: &Value| native_dump_props(ctx, v);
        print_r_buf(&mut buf, &args[0], 0, &mut Seen::new(), &mut native)?;
    }
    if return_mode {
        Ok(Value::Str(Str::from_vec(buf)))
    } else {
        ctx.out().extend_from_slice(&buf);
        Ok(Value::Bool(true))
    }
}

// ---- var_dump ---------------------------------------------------------------

/// What an object's dump shows besides, or instead of, its slots — php's
/// `get_debug_info`.
enum DebugTable {
    /// Appended after the standard properties: a native class's property
    /// table or its declared names, when it has no `debug` hook.
    Extra(Vec<(ArrayKey, Value)>),
    /// The whole table: what `__debugInfo()` returned (the SPL containers
    /// answer it natively with their standard properties plus the private
    /// state php shows), a native class's `debug` hook (the DOM's
    /// `prop_handler`s after the standard properties, an object-valued
    /// entry already replaced by `(object value omitted)`), or a closure's
    /// debug table. String keys may be mangled (`"\0Class\0name"`) and
    /// print as that visibility.
    Whole(Vec<(ArrayKey, Value)>),
}

/// The hook a dump asks for an object's [`DebugTable`].
type NativeDump<'a> = &'a mut dyn FnMut(&Value) -> Result<Option<DebugTable>, Unwind>;

/// An object's debug table: `__debugInfo()` when the class has one —
/// php's `zend_std_get_debug_info`: an array is the table, `null` a
/// deprecation and an empty one, anything else the fatal error — else a
/// native class's computed table; a closure's debug table.
fn native_dump_props(ctx: &mut Ctx, v: &Value) -> Result<Option<DebugTable>, Unwind> {
    match v {
        // An uninitialized lazy object is dumped as it is; php does not
        // initialize it for a dump either.
        Value::Object(o) if !o.is_lazy() && ctx.class_of(o).magic.contains(MagicFlags::DEBUGINFO) => {
            let o = o.clone();
            let r = ctx.call_method(&o, b"__debugInfo", &[])?;
            match &*r.deref() {
                Value::Array(a) => Ok(Some(DebugTable::Whole(
                    a.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
                ))),
                Value::Null => {
                    let msg = format!(
                        "Returning null from {}::__debugInfo() is deprecated, return an empty array instead",
                        ctx.class_name_of(&o)
                    );
                    ctx.deprecated(&msg)?;
                    Ok(Some(DebugTable::Whole(Vec::new())))
                }
                _ => Err(ctx.fatal("__debuginfo() must return an array")),
            }
        }
        Value::Object(o) => Ok(ctx.native_debug_table(o).map(|(whole, t)| {
            if whole {
                DebugTable::Whole(t)
            } else {
                DebugTable::Extra(t)
            }
        })),
        Value::Closure(c) => Ok(Some(DebugTable::Whole(ctx.closure_debug_info(c)))),
        _ => Ok(None),
    }
}

/// php's `zend_unmangle_property_name`: a key of the form
/// `"\0Class\0name"` is a private property of `Class`, `"\0*\0name"` a
/// protected one; anything else is the plain name.
fn unmangle(key: &[u8]) -> (&[u8], Option<&[u8]>) {
    if key.len() < 3 || key[0] != 0 || key[1] == 0 {
        return (key, None);
    }
    let Some(end) = key[1..].iter().position(|&b| b == 0) else {
        return (key, None);
    };
    (&key[end + 2..], Some(&key[1..=end]))
}

/// One property of an object, copied out before the dump recurses: an
/// element's `__debugInfo()` may touch this object, and a nested value
/// may be a handle back onto it.
struct Slot {
    name: Box<[u8]>,
    value: Value,
    vis: Vis,
    /// The declaring class of a private slot (`["p":"Decl":private]`).
    decl: Option<std::rc::Rc<[u8]>>,
    /// The declared type of an uninitialized typed slot.
    ty: Option<std::rc::Rc<str>>,
    /// For a reference slot: whether another handle shared the cell
    /// *before* this copy took one — the `&` marker's condition.
    shared: bool,
}

impl Slot {
    fn new(name: Box<[u8]>, value: &Value, vis: Vis) -> Slot {
        let shared = matches!(value, Value::Ref(r) if r.strong_count() > 1);
        Slot { name, value: value.clone(), vis, decl: None, ty: None, shared }
    }
}

/// The standard property list of `o` as a dump shows it: an uninitialized
/// *typed* slot is kept (it prints `uninitialized(T)`), an untyped one that
/// was `unset()` is gone.
fn slots_of(o: &Object) -> Vec<Slot> {
    o.with_data(|d| {
        d.props_in_order()
            .filter_map(|p| {
                let ty = p.meta.and_then(|m| m.ty.clone());
                if p.value.is_uninit() && ty.is_none() {
                    return None;
                }
                let mut slot = Slot::new(Box::from(p.name), p.value, p.vis);
                slot.decl = p.meta.map(|m| m.decl_class_name.clone());
                slot.ty = ty;
                Some(slot)
            })
            .collect()
    })
}

/// A debug-table entry as a [`Slot`]: an integer key prints as `[0]=>`, a
/// mangled string key as the visibility it encodes.
fn table_slot(k: &ArrayKey, v: &Value) -> (Option<i64>, Slot) {
    match k {
        ArrayKey::Int(i) => (Some(*i), Slot::new(Box::default(), v, Vis::Public)),
        ArrayKey::Str(key) => {
            let (name, class) = unmangle(key);
            let (vis, decl) = match class {
                Some(b"*") => (Vis::Protected, None),
                Some(c) => (Vis::Private, Some(std::rc::Rc::from(c))),
                None => (Vis::Public, None),
            };
            let mut slot = Slot::new(Box::from(name), v, vis);
            slot.decl = decl;
            (None, slot)
        }
    }
}

fn indent(out: &mut Vec<u8>, spaces: usize) {
    out.resize(out.len() + spaces, b' ');
}

fn dump_float(out: &mut Vec<u8>, f: f64, precision: i64) {
    out.extend_from_slice(php_gcvt(f, precision, b'E').as_bytes());
}

/// The `serialize_precision` ini value as `var_dump` / `var_export` /
/// `serialize` / `json_encode` consume it: `-1` for the shortest round trip,
/// otherwise the number of significant digits (`0` meaning php's
/// `FLOAT_DIGITS`, 6).
pub(crate) fn serialize_precision(ctx: &Ctx) -> i64 {
    match ctx.ini.int("serialize_precision") {
        0 => 6,
        p => p,
    }
}

/// php-src `php_gcvt(value, ndigit, '.', exp_char)`: with `precision == -1`
/// (`zend_dtoa` mode 0) the shortest round-trip digit string, otherwise
/// (mode 2) the value correctly rounded to `precision` significant digits
/// with trailing zeros dropped — laid out the way `serialize_precision`
/// prints it: fixed notation while the decimal point lands within
/// `-3 ..= ndigit` digits of the first significant one (`ndigit` = 17 for
/// the shortest form), otherwise `d.dddE+N` with a signed exponent and a
/// forced `.0` mantissa for a single digit. Shared by `var_dump` (`E`),
/// `var_export`/`serialize` (`E`) and available to `json_encode` (`e`).
/// Non-finite values print `NAN`, `INF`, `-INF`; zero keeps its sign
/// (`-0`).
pub(crate) fn php_gcvt(f: f64, precision: i64, exp_char: u8) -> String {
    if f.is_nan() {
        return "NAN".to_string();
    }
    if f.is_infinite() {
        return if f < 0.0 { "-INF" } else { "INF" }.to_string();
    }
    if f == 0.0 {
        return if f.is_sign_negative() { "-0" } else { "0" }.to_string();
    }
    // Rust's `{:e}` (no precision) yields the shortest round-tripping mantissa
    // and a bare exponent — the same digit string `zend_dtoa(mode 0)`
    // produces; with a precision it is the correctly rounded `mode 2` string.
    let ndigit: i32 = if precision < 0 { 17 } else { precision.clamp(1, 500) as i32 };
    let sci = if precision < 0 {
        format!("{:e}", f.abs())
    } else {
        format!("{:.*e}", (ndigit - 1) as usize, f.abs())
    };
    let (mant, exp_str) = sci.split_once('e').expect("LowerExp always has 'e'");
    let exp: i32 = exp_str.parse().expect("valid exponent");
    let mut digits: Vec<u8> = mant.bytes().filter(|&c| c != b'.').collect();
    while digits.len() > 1 && digits.last() == Some(&b'0') {
        digits.pop();
    }
    // dtoa's `decpt`: the decimal point sits this many digits from the start.
    let decpt = exp + 1;
    let mut out = String::new();
    if f < 0.0 {
        out.push('-');
    }
    if !(-3..=ndigit).contains(&decpt) {
        // Exponential: "d.ddd" then exp_char, sign, exponent.
        let e = decpt - 1;
        out.push(digits[0] as char);
        out.push('.');
        if digits.len() == 1 {
            out.push('0');
        } else {
            out.extend(digits[1..].iter().map(|&b| b as char));
        }
        out.push(exp_char as char);
        out.push(if e < 0 { '-' } else { '+' });
        out.push_str(&e.unsigned_abs().to_string());
    } else if decpt < 0 {
        // Pure fraction: "0.00…digits".
        out.push_str("0.");
        for _ in 0..(-decpt) {
            out.push('0');
        }
        out.extend(digits.iter().map(|&b| b as char));
    } else {
        // Integer part padded with zeros past the digit string, then any
        // fractional remainder.
        let dp = decpt as usize;
        let mut src = 0usize;
        for _ in 0..dp {
            if src < digits.len() {
                out.push(digits[src] as char);
                src += 1;
            } else {
                out.push('0');
            }
        }
        if src < digits.len() {
            if src == 0 {
                out.push('0');
            }
            out.push('.');
            out.extend(digits[src..].iter().map(|&b| b as char));
        }
    }
    out
}

/// Emit the `var_dump` representation of `v`. `pad` is the indentation (in
/// spaces) of the *enclosing* container; the caller has already written `pad`
/// spaces before the value when this is an element.
fn dump(
    out: &mut Vec<u8>,
    v: &Value,
    pad: usize,
    seen: &mut Seen,
    precision: i64,
    native: NativeDump<'_>,
) -> Result<(), Unwind> {
    match v {
        // An uninitialized typed property never reaches user code (the runtime
        // errors first) and is skipped inside objects; standalone it is null.
        Value::Null | Value::Uninit => out.extend_from_slice(b"NULL\n"),
        Value::Bool(b) => {
            out.extend_from_slice(if *b { b"bool(true)\n" } else { b"bool(false)\n" });
        }
        Value::Int(i) => {
            out.extend_from_slice(format!("int({i})\n").as_bytes());
        }
        Value::Float(f) => {
            out.extend_from_slice(b"float(");
            dump_float(out, *f, precision);
            out.extend_from_slice(b")\n");
        }
        Value::Str(s) => {
            out.extend_from_slice(format!("string({}) \"", s.len()).as_bytes());
            out.extend_from_slice(s.as_bytes());
            out.extend_from_slice(b"\"\n");
        }
        Value::Array(a) => {
            out.extend_from_slice(format!("array({}) {{\n", a.len()).as_bytes());
            for (k, val) in a.iter() {
                indent(out, pad + 2);
                dump_key(out, k);
                indent(out, pad + 2);
                dump(out, val, pad + 2, seen, precision, native)?;
            }
            indent(out, pad);
            out.extend_from_slice(b"}\n");
        }
        // A closure is an object whose "properties" are its debug table.
        Value::Closure(c) => {
            if seen.objects.contains(&c.id()) {
                out.extend_from_slice(b"*RECURSION*\n");
                return Ok(());
            }
            let props = match native(v)? {
                Some(DebugTable::Whole(t) | DebugTable::Extra(t)) => t,
                None => Vec::new(),
            };
            out.extend_from_slice(format!("object(Closure)#{} ({}) {{\n", c.id(), props.len()).as_bytes());
            seen.objects.push(c.id());
            for (k, val) in &props {
                indent(out, pad + 2);
                dump_key(out, k);
                indent(out, pad + 2);
                dump(out, val, pad + 2, seen, precision, native)?;
            }
            seen.objects.pop();
            indent(out, pad);
            out.extend_from_slice(b"}\n");
        }
        Value::Object(o) => {
            // The cycle check comes before `__debugInfo()` (php protects
            // the object first, then asks for its table).
            if seen.objects.contains(&o.id()) {
                out.extend_from_slice(b"*RECURSION*\n");
                return Ok(());
            }
            let table = native(v)?;
            dump_object(out, o, pad, seen, precision, table, native)?;
        }
        // PHP marks a reference `&` only while more than one handle shares it
        // (measured before the cycle guard takes its own handle).
        Value::Ref(r) => dump_ref(out, r, r.strong_count() > 1, pad, seen, precision, native)?,
        Value::Resource(r) => {
            out.extend_from_slice(format!("resource({}) of type ({})\n", r.id(), r.kind()).as_bytes());
        }
    }
    Ok(())
}

/// A reference cell: `&` when `shared`, `*RECURSION*` on a cycle.
fn dump_ref(
    out: &mut Vec<u8>,
    r: &PhpRef,
    shared: bool,
    pad: usize,
    seen: &mut Seen,
    precision: i64,
    native: NativeDump<'_>,
) -> Result<(), Unwind> {
    if !seen.enter_ref(r) {
        out.extend_from_slice(b"*RECURSION*\n");
        return Ok(());
    }
    let inner = r.get();
    // php prints the `&` with the type, never before `*RECURSION*`.
    let recursive = matches!(&inner, Value::Object(o) if seen.objects.contains(&o.id()));
    if shared && !recursive {
        out.push(b'&');
    }
    dump(out, &inner, pad, seen, precision, native)?;
    seen.refs.pop();
    Ok(())
}

/// `object(Class)#id (count) { ["name"(:protected | :"Decl":private)]=> value … }`
#[allow(clippy::too_many_arguments)]
fn dump_object(
    out: &mut Vec<u8>,
    o: &Object,
    pad: usize,
    seen: &mut Seen,
    precision: i64,
    table: Option<DebugTable>,
    native: NativeDump<'_>,
) -> Result<(), Unwind> {
    let class_name = o.layout().class_name().to_vec();
    // php prints an enum case as `enum(Suit::Hearts)`, with no id and no
    // property list.
    if o.flags().contains(rphp_value::ObjFlags::ENUM_CASE) {
        out.extend_from_slice(b"enum(");
        out.extend_from_slice(display_class_name(&class_name));
        out.extend_from_slice(b"::");
        if let Some(v) = o.get_deref(b"name") {
            out.extend_from_slice(&v.to_php_bytes());
        }
        out.extend_from_slice(b")\n");
        return Ok(());
    }
    // A php 8.4 lazy object is headed `lazy ghost ` / `lazy proxy ` while
    // uninitialized (a proxy keeps the prefix for good), and an initialized
    // proxy shows the real instance under `["instance"]`.
    let lazy = o.lazy();
    match lazy.as_ref().map(|l| (l.kind, l.initialized)) {
        Some((rphp_value::LazyKind::Ghost, false)) => out.extend_from_slice(b"lazy ghost "),
        Some((rphp_value::LazyKind::Proxy, _)) => out.extend_from_slice(b"lazy proxy "),
        _ => {}
    }
    out.extend_from_slice(b"object(");
    out.extend_from_slice(display_class_name(&class_name));
    if let Some(real) = lazy.and_then(|l| l.real.clone()) {
        out.extend_from_slice(format!(")#{} (1) {{\n", o.id()).as_bytes());
        seen.objects.push(o.id());
        indent(out, pad + 2);
        out.extend_from_slice(b"[\"instance\"]=>\n");
        indent(out, pad + 2);
        dump(out, &Value::Object(real), pad + 2, seen, precision, native)?;
        seen.objects.pop();
        indent(out, pad);
        out.extend_from_slice(b"}\n");
        return Ok(());
    }
    // The standard properties, then a native class's computed ones — or
    // the `__debugInfo()` table alone.
    let entries: Vec<(Option<i64>, Slot)> = match table {
        Some(DebugTable::Whole(t)) => t.iter().map(|(k, v)| table_slot(k, v)).collect(),
        Some(DebugTable::Extra(t)) => slots_of(o)
            .into_iter()
            .map(|s| (None, s))
            .chain(t.iter().map(|(k, v)| table_slot(k, v)))
            .collect(),
        None => slots_of(o).into_iter().map(|s| (None, s)).collect(),
    };
    // php counts the property table, which an uninitialized typed slot is
    // not in (it is still listed, as `uninitialized(T)`).
    let count = entries.iter().filter(|(_, p)| !p.value.is_uninit()).count();
    out.extend_from_slice(format!(")#{} ({}) {{\n", o.id(), count).as_bytes());
    seen.objects.push(o.id());
    for (index, p) in &entries {
        indent(out, pad + 2);
        match index {
            Some(i) => out.extend_from_slice(format!("[{i}]=>\n").as_bytes()),
            None => {
                out.extend_from_slice(b"[\"");
                out.extend_from_slice(&p.name);
                out.push(b'"');
                match p.vis {
                    Vis::Public => {}
                    Vis::Protected => out.extend_from_slice(b":protected"),
                    Vis::Private => {
                        out.extend_from_slice(b":\"");
                        out.extend_from_slice(p.decl.as_deref().unwrap_or(b""));
                        out.extend_from_slice(b"\":private");
                    }
                }
                out.extend_from_slice(b"]=>\n");
            }
        }
        indent(out, pad + 2);
        match (&p.ty, &p.value) {
            (Some(ty), v) if v.is_uninit() => {
                out.extend_from_slice(format!("uninitialized({ty})\n").as_bytes());
            }
            // The slot's own copy of the cell must not count as a sharer.
            (_, Value::Ref(r)) => dump_ref(out, r, p.shared, pad + 2, seen, precision, native)?,
            (_, v) => dump(out, v, pad + 2, seen, precision, native)?,
        }
    }
    seen.objects.pop();
    indent(out, pad);
    out.extend_from_slice(b"}\n");
    Ok(())
}

fn dump_key(out: &mut Vec<u8>, k: &ArrayKey) {
    match k {
        ArrayKey::Int(i) => out.extend_from_slice(format!("[{i}]=>\n").as_bytes()),
        ArrayKey::Str(b) => {
            out.extend_from_slice(b"[\"");
            out.extend_from_slice(b);
            out.extend_from_slice(b"\"]=>\n");
        }
    }
}

// ---- print_r ----------------------------------------------------------------

/// Emit the `print_r` representation of `v`. `pad` is the indentation applied to
/// the `(` / `)` lines of a container; scalars print their plain string cast.
fn print_r_buf(
    out: &mut Vec<u8>,
    v: &Value,
    pad: usize,
    seen: &mut Seen,
    native: NativeDump<'_>,
) -> Result<(), Unwind> {
    match v {
        Value::Array(a) => {
            out.extend_from_slice(b"Array\n");
            indent(out, pad);
            out.extend_from_slice(b"(\n");
            for (k, val) in a.iter() {
                indent(out, pad + 4);
                out.push(b'[');
                match k {
                    ArrayKey::Int(i) => out.extend_from_slice(i.to_string().as_bytes()),
                    ArrayKey::Str(b) => out.extend_from_slice(b),
                }
                out.extend_from_slice(b"] => ");
                print_r_buf(out, val, pad + 8, seen, native)?;
                out.push(b'\n');
            }
            indent(out, pad);
            out.extend_from_slice(b")\n");
        }
        Value::Object(o) => {
            out.extend_from_slice(display_class_name(o.layout().class_name()));
            // php heads an enum case with `Enum` (pure) or `Enum:int`/
            // `Enum:string` (backed) instead of `Object`, then lists its
            // properties as usual.
            if o.flags().contains(rphp_value::ObjFlags::ENUM_CASE) {
                out.extend_from_slice(b" Enum");
                match o.get_deref(b"value") {
                    Some(Value::Int(_)) => out.extend_from_slice(b":int"),
                    Some(Value::Str(_)) => out.extend_from_slice(b":string"),
                    _ => {}
                }
                out.push(b'\n');
            } else {
                out.extend_from_slice(b" Object\n");
            }
            if seen.objects.contains(&o.id()) {
                out.extend_from_slice(b" *RECURSION*");
                return Ok(());
            }
            let table = native(v)?;
            print_r_object(out, o, pad, seen, table, native)?;
        }
        Value::Closure(c) => {
            out.extend_from_slice(b"Closure Object\n");
            if seen.objects.contains(&c.id()) {
                out.extend_from_slice(b" *RECURSION*");
                return Ok(());
            }
            let props = match native(v)? {
                Some(DebugTable::Whole(t) | DebugTable::Extra(t)) => t,
                None => Vec::new(),
            };
            indent(out, pad);
            out.extend_from_slice(b"(\n");
            seen.objects.push(c.id());
            for (k, val) in &props {
                indent(out, pad + 4);
                out.push(b'[');
                match k {
                    ArrayKey::Int(i) => out.extend_from_slice(i.to_string().as_bytes()),
                    ArrayKey::Str(b) => out.extend_from_slice(b),
                }
                out.extend_from_slice(b"] => ");
                print_r_buf(out, val, pad + 8, seen, native)?;
                out.push(b'\n');
            }
            seen.objects.pop();
            indent(out, pad);
            out.extend_from_slice(b")\n");
        }
        Value::Ref(r) => {
            if !seen.enter_ref(r) {
                out.extend_from_slice(b"Array\n *RECURSION*");
                return Ok(());
            }
            let inner = r.get();
            print_r_buf(out, &inner, pad, seen, native)?;
            seen.refs.pop();
        }
        // Scalars (and resources: `Resource id #N`) use the same string
        // conversion as `echo`.
        _ => v.append_php_bytes(out),
    }
    Ok(())
}

/// `( [name(:protected | :Decl:private)] => value … )` — the body after the
/// `Class Object` heading the caller wrote.
fn print_r_object(
    out: &mut Vec<u8>,
    o: &Object,
    pad: usize,
    seen: &mut Seen,
    table: Option<DebugTable>,
    native: NativeDump<'_>,
) -> Result<(), Unwind> {
    indent(out, pad);
    out.extend_from_slice(b"(\n");
    seen.objects.push(o.id());
    // An initialized lazy proxy lists the real instance under `[instance]`.
    if let Some(real) = o.lazy().and_then(|l| l.real.clone()) {
        indent(out, pad + 4);
        out.extend_from_slice(b"[instance] => ");
        print_r_buf(out, &Value::Object(real), pad + 8, seen, native)?;
        out.push(b'\n');
        seen.objects.pop();
        indent(out, pad);
        out.extend_from_slice(b")\n");
        return Ok(());
    }
    // The standard properties (an uninitialized slot is not listed), then
    // a native class's computed ones — or the `__debugInfo()` table alone.
    let std = || slots_of(o).into_iter().filter(|s| !s.value.is_uninit()).map(|s| (None, s));
    let entries: Vec<(Option<i64>, Slot)> = match table {
        Some(DebugTable::Whole(t)) => t.iter().map(|(k, v)| table_slot(k, v)).collect(),
        Some(DebugTable::Extra(t)) => std().chain(t.iter().map(|(k, v)| table_slot(k, v))).collect(),
        None => std().collect(),
    };
    for (index, p) in &entries {
        indent(out, pad + 4);
        out.push(b'[');
        match index {
            Some(i) => out.extend_from_slice(i.to_string().as_bytes()),
            None => {
                out.extend_from_slice(&p.name);
                match p.vis {
                    Vis::Public => {}
                    Vis::Protected => out.extend_from_slice(b":protected"),
                    Vis::Private => {
                        out.push(b':');
                        out.extend_from_slice(p.decl.as_deref().unwrap_or(b""));
                        out.extend_from_slice(b":private");
                    }
                }
            }
        }
        out.extend_from_slice(b"] => ");
        print_r_buf(out, &p.value, pad + 8, seen, native)?;
        out.push(b'\n');
    }
    seen.objects.pop();
    indent(out, pad);
    out.extend_from_slice(b")\n");
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::rc::Rc;

    use rphp_value::{Layout, Object, PhpRef, PropMeta, Resource};

    use super::*;

    fn dumped(v: &Value) -> String {
        let mut out = Vec::new();
        dump(&mut out, v, 0, &mut Seen::new(), -1, &mut |_| Ok(None)).unwrap();
        String::from_utf8(out).unwrap()
    }

    fn printed(v: &Value) -> String {
        let mut out = Vec::new();
        print_r_buf(&mut out, v, 0, &mut Seen::new(), &mut |_| Ok(None)).unwrap();
        String::from_utf8(out).unwrap()
    }

    fn meta(name: &str, vis: Vis, decl: &str) -> PropMeta {
        PropMeta {
            name: Box::from(name.as_bytes()),
            vis,
            decl_class: 0,
            decl_class_name: Rc::from(decl.as_bytes()),
            ty: None,
            is_virtual: false,
        }
    }

    /// `class Foo { public $a = 1; protected $b = 2; private $c = 3; }` plus a
    /// dynamic `$d = 4` — object id 1.
    fn foo() -> Object {
        let layout = Layout::new(
            Rc::from(&b"Foo"[..]),
            vec![
                meta("a", Vis::Public, "Foo"),
                meta("b", Vis::Protected, "Foo"),
                meta("c", Vis::Private, "Foo"),
            ],
        );
        let o = Object::new(0, 1, Rc::new(layout), vec![Value::Int(1), Value::Int(2), Value::Int(3)]);
        o.set(b"d", Value::Int(4));
        o
    }

    #[test]
    fn var_dump_object_matches_php() {
        let expected = "object(Foo)#1 (4) {\n  [\"a\"]=>\n  int(1)\n  [\"b\":protected]=>\n  int(2)\n  [\"c\":\"Foo\":private]=>\n  int(3)\n  [\"d\"]=>\n  int(4)\n}\n";
        assert_eq!(dumped(&Value::Object(foo())), expected);
    }

    #[test]
    fn print_r_object_matches_php_including_nesting() {
        let expected = "Foo Object\n(\n    [a] => 1\n    [b:protected] => 2\n    [c:Foo:private] => 3\n    [d] => 4\n)\n";
        assert_eq!(printed(&Value::Object(foo())), expected);
        // Nested in an array: the same shape as a nested array.
        let mut arr = rphp_value::Array::new();
        arr.push(Value::Object(foo()));
        let expected = "Array\n(\n    [0] => Foo Object\n        (\n            [a] => 1\n            [b:protected] => 2\n            [c:Foo:private] => 3\n            [d] => 4\n        )\n\n)\n";
        assert_eq!(printed(&Value::Array(arr)), expected);
    }

    #[test]
    fn self_referential_object_prints_recursion_marker() {
        // class Foo { public $self; public $n = 1; } $o->self = $o;
        let layout = Layout::new(
            Rc::from(&b"Foo"[..]),
            vec![meta("self", Vis::Public, "Foo"), meta("n", Vis::Public, "Foo")],
        );
        let o = Object::new(0, 1, Rc::new(layout), vec![Value::Null, Value::Int(1)]);
        o.set(b"self", Value::Object(o.clone()));
        assert_eq!(
            dumped(&Value::Object(o.clone())),
            "object(Foo)#1 (2) {\n  [\"self\"]=>\n  *RECURSION*\n  [\"n\"]=>\n  int(1)\n}\n"
        );
        assert_eq!(
            printed(&Value::Object(o.clone())),
            "Foo Object\n(\n    [self] => Foo Object\n *RECURSION*\n    [n] => 1\n)\n"
        );
        // Break the cycle so the test does not leak.
        o.set(b"self", Value::Null);
    }

    #[test]
    fn float_dumps_use_the_serialize_precision_layout() {
        let d = |f: f64| dumped(&Value::Float(f));
        assert_eq!(d(1.0), "float(1)\n");
        assert_eq!(d(-0.0), "float(-0)\n");
        assert_eq!(d(0.1 + 0.2), "float(0.30000000000000004)\n");
        assert_eq!(d(1e15), "float(1000000000000000)\n");
        assert_eq!(d(1e17), "float(1.0E+17)\n");
        assert_eq!(d(1e25), "float(1.0E+25)\n");
        assert_eq!(d(1e-7), "float(1.0E-7)\n");
        assert_eq!(d(0.0001), "float(0.0001)\n");
        assert_eq!(d(123456789012345678.0), "float(1.2345678901234568E+17)\n");
        assert_eq!(d(f64::NAN), "float(NAN)\n");
        assert_eq!(d(f64::NEG_INFINITY), "float(-INF)\n");
        assert_eq!(php_gcvt(1e25, -1, b'e'), "1.0e+25");
        // serialize_precision=17 / 14 / 6: correctly rounded, zeros dropped.
        assert_eq!(php_gcvt(0.1, 17, b'E'), "0.10000000000000001");
        assert_eq!(php_gcvt(0.1, 14, b'E'), "0.1");
        assert_eq!(php_gcvt(1e14, 14, b'E'), "1.0E+14");
        assert_eq!(php_gcvt(4872401723.124452, 10, b'E'), "4872401723");
        assert_eq!(php_gcvt(1.0 / 3.0, 6, b'E'), "0.333333");
        assert_eq!(php_gcvt(123456.0, 3, b'E'), "1.23E+5");
    }

    #[test]
    fn self_referential_arrays_do_not_recurse_forever() {
        // $a = []; $a[0] = &$a; $a[1] = 1;
        let cell = PhpRef::new(Value::empty_array());
        cell.update(|v| {
            if let Value::Array(a) = v {
                a.set_ref(ArrayKey::Int(0), cell.clone());
                a.push(Value::Int(1));
            }
        });
        let a = cell.get();
        let out = dumped(&a);
        assert!(out.contains("*RECURSION*"), "{out}");
        assert!(printed(&a).contains("*RECURSION*"));
        // Break the cycle.
        cell.set(Value::Null);
    }

    #[test]
    fn references_and_resources_dump_like_php() {
        // $a = [1, 2]; $r = &$a[0]; var_dump($a);  =>  [0]=> &int(1)
        let mut a = rphp_value::Array::new();
        a.push(Value::Int(1));
        a.push(Value::Int(2));
        let r = a.get_ref(ArrayKey::Int(0));
        assert_eq!(dumped(&Value::Array(a.clone())), "array(2) {\n  [0]=>\n  &int(1)\n  [1]=>\n  int(2)\n}\n");
        assert_eq!(printed(&Value::Array(a.clone())), "Array\n(\n    [0] => 1\n    [1] => 2\n)\n");
        // unset($r): a lone handle prints as a plain value.
        drop(r);
        assert_eq!(dumped(&Value::Array(a)), "array(2) {\n  [0]=>\n  int(1)\n  [1]=>\n  int(2)\n}\n");
        let lone = Value::Ref(PhpRef::new(Value::string(b"x")));
        assert_eq!(dumped(&lone), "string(1) \"x\"\n");

        let res = Resource::new(1, "stream", Box::new(()));
        let v = Value::Resource(res.clone());
        assert_eq!(dumped(&v), "resource(1) of type (stream)\n");
        assert_eq!(printed(&v), "Resource id #1");
        res.close();
        assert_eq!(dumped(&v), "resource(1) of type (Unknown)\n");
    }
}
