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
use rphp_value::{
    display_class_name, ArrayKey, ObjectData, PhpRef, PropEntry, Str, Value, Vis,
};

use rphp_runtime::{Ctx, NativeFn, NativeResult, nf};

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
            dump(&mut buf, v, 0, &mut seen, precision, &mut native);
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
        print_r_buf(&mut buf, &args[0], 0, &mut Seen::new(), &mut native);
    }
    if return_mode {
        Ok(Value::Str(Str::from_vec(buf)))
    } else {
        ctx.out().extend_from_slice(&buf);
        Ok(Value::Bool(true))
    }
}

// ---- var_dump ---------------------------------------------------------------

/// The computed properties a native class contributes to a dump (php's
/// `get_debug_info` for the DOM's `prop_handler`s): name → value, an
/// object-valued one already replaced by `(object value omitted)`.
type NativeDump<'a> = &'a mut dyn FnMut(&Value) -> Option<Vec<(ArrayKey, Value)>>;

/// What a native class's computed properties — or a closure's debug
/// table — look like in a dump.
fn native_dump_props(ctx: &mut Ctx, v: &Value) -> Option<Vec<(ArrayKey, Value)>> {
    match v {
        Value::Object(o) => ctx.native_debug_table(o),
        Value::Closure(c) => Some(ctx.closure_debug_info(c)),
        _ => None,
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
) {
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
                dump(out, val, pad + 2, seen, precision, native);
            }
            indent(out, pad);
            out.extend_from_slice(b"}\n");
        }
        // A closure is an object whose "properties" are its debug table.
        Value::Closure(c) => {
            if seen.objects.contains(&c.id()) {
                out.extend_from_slice(b"*RECURSION*\n");
                return;
            }
            let props = native(v).unwrap_or_default();
            out.extend_from_slice(format!("object(Closure)#{} ({}) {{\n", c.id(), props.len()).as_bytes());
            seen.objects.push(c.id());
            for (k, val) in &props {
                indent(out, pad + 2);
                dump_key(out, k);
                indent(out, pad + 2);
                dump(out, val, pad + 2, seen, precision, native);
            }
            seen.objects.pop();
            indent(out, pad);
            out.extend_from_slice(b"}\n");
        }
        Value::Object(o) => {
            let extra = native(v).unwrap_or_default();
            o.with_data(|d| dump_object(out, d, pad, seen, precision, extra, native))
        }
        // PHP marks a reference `&` only while more than one handle shares it.
        Value::Ref(r) => {
            // (Measured before the cycle guard takes its own handle.)
            let shared = r.strong_count() > 1;
            if !seen.enter_ref(r) {
                out.extend_from_slice(b"*RECURSION*\n");
                return;
            }
            let inner = r.get();
            // php prints the `&` with the type, never before `*RECURSION*`.
            let recursive = matches!(&inner, Value::Object(o) if seen.objects.contains(&o.id()));
            if shared && !recursive {
                out.push(b'&');
            }
            dump(out, &inner, pad, seen, precision, native);
            seen.refs.pop();
        }
        Value::Resource(r) => {
            out.extend_from_slice(format!("resource({}) of type ({})\n", r.id(), r.kind()).as_bytes());
        }
    }
}

/// `object(Class)#id (count) { ["name"(:protected | :"Decl":private)]=> value … }`
fn dump_object(
    out: &mut Vec<u8>,
    d: &ObjectData,
    pad: usize,
    seen: &mut Seen,
    precision: i64,
    extra: Vec<(ArrayKey, Value)>,
    native: NativeDump<'_>,
) {
    if seen.objects.contains(&d.id()) {
        out.extend_from_slice(b"*RECURSION*\n");
        return;
    }
    // php prints an enum case as `enum(Suit::Hearts)`, with no id and no
    // property list.
    if d.flags().contains(rphp_value::ObjFlags::ENUM_CASE) {
        out.extend_from_slice(b"enum(");
        out.extend_from_slice(display_class_name(d.layout().class_name()));
        out.extend_from_slice(b"::");
        if let Some(p) = d.props_in_order().find(|p| p.name == b"name") {
            out.extend_from_slice(&p.value.to_php_bytes());
        }
        out.extend_from_slice(b")\n");
        return;
    }
    // A php 8.4 lazy object is headed `lazy ghost ` / `lazy proxy ` while
    // uninitialized (a proxy keeps the prefix for good), and an initialized
    // proxy shows the real instance under `["instance"]`.
    let lazy = d.lazy();
    match lazy.map(|l| (l.kind, l.initialized)) {
        Some((rphp_value::LazyKind::Ghost, false)) => out.extend_from_slice(b"lazy ghost "),
        Some((rphp_value::LazyKind::Proxy, _)) => out.extend_from_slice(b"lazy proxy "),
        _ => {}
    }
    out.extend_from_slice(b"object(");
    out.extend_from_slice(display_class_name(d.layout().class_name()));
    if let Some(real) = lazy.and_then(|l| l.real.clone()) {
        out.extend_from_slice(format!(")#{} (1) {{\n", d.id()).as_bytes());
        seen.objects.push(d.id());
        indent(out, pad + 2);
        out.extend_from_slice(b"[\"instance\"]=>\n");
        indent(out, pad + 2);
        dump(out, &Value::Object(real), pad + 2, seen, precision, native);
        seen.objects.pop();
        indent(out, pad);
        out.extend_from_slice(b"}\n");
        return;
    }
    out.extend_from_slice(
        format!(")#{} ({}) {{\n", d.id(), d.prop_count() + extra.len()).as_bytes(),
    );
    seen.objects.push(d.id());
    // A native class's computed properties come first, as php's
    // `get_debug_info` lists them.
    for (k, v) in &extra {
        indent(out, pad + 2);
        dump_key(out, k);
        indent(out, pad + 2);
        dump(out, v, pad + 2, seen, precision, native);
    }
    for p in d.props_in_order() {
        // An uninitialized *typed* slot prints as `uninitialized(T)` (a
        // lazy object's slots all are, until its initializer runs); an
        // untyped one that was unset is simply gone.
        let ty = p.meta.and_then(|m| m.ty.clone());
        if p.value.is_uninit() && ty.is_none() {
            continue;
        }
        indent(out, pad + 2);
        out.extend_from_slice(b"[\"");
        out.extend_from_slice(p.name);
        out.push(b'"');
        match p.vis {
            Vis::Public => {}
            Vis::Protected => out.extend_from_slice(b":protected"),
            Vis::Private => {
                out.extend_from_slice(b":\"");
                out.extend_from_slice(decl_class_name(&p));
                out.extend_from_slice(b"\":private");
            }
        }
        out.extend_from_slice(b"]=>\n");
        indent(out, pad + 2);
        match ty {
            Some(ty) if p.value.is_uninit() => {
                out.extend_from_slice(format!("uninitialized({ty})\n").as_bytes());
            }
            _ => dump(out, p.value, pad + 2, seen, precision, native),
        }
    }
    seen.objects.pop();
    indent(out, pad);
    out.extend_from_slice(b"}\n");
}

/// The class declaring a private property (dynamic properties are public, so
/// this is only consulted for declared slots).
fn decl_class_name<'a>(p: &PropEntry<'a>) -> &'a [u8] {
    p.meta.map_or(b"", |m| &m.decl_class_name)
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
fn print_r_buf(out: &mut Vec<u8>, v: &Value, pad: usize, seen: &mut Seen, native: NativeDump<'_>) {
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
                print_r_buf(out, val, pad + 8, seen, native);
                out.push(b'\n');
            }
            indent(out, pad);
            out.extend_from_slice(b")\n");
        }
        Value::Object(o) => {
            let extra = native(v).unwrap_or_default();
            o.with_data(|d| print_r_object(out, d, pad, seen, extra, native))
        }
        Value::Closure(c) => {
            out.extend_from_slice(b"Closure Object\n");
            if seen.objects.contains(&c.id()) {
                out.extend_from_slice(b" *RECURSION*");
                return;
            }
            let props = native(v).unwrap_or_default();
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
                print_r_buf(out, val, pad + 8, seen, native);
                out.push(b'\n');
            }
            seen.objects.pop();
            indent(out, pad);
            out.extend_from_slice(b")\n");
        }
        Value::Ref(r) => {
            if !seen.enter_ref(r) {
                out.extend_from_slice(b"Array\n *RECURSION*");
                return;
            }
            let inner = r.get();
            print_r_buf(out, &inner, pad, seen, native);
            seen.refs.pop();
        }
        // Scalars (and resources: `Resource id #N`) use the same string
        // conversion as `echo`.
        _ => v.append_php_bytes(out),
    }
}

/// `Class Object ( [name(:protected | :Decl:private)] => value … )`
fn print_r_object(
    out: &mut Vec<u8>,
    d: &ObjectData,
    pad: usize,
    seen: &mut Seen,
    extra: Vec<(ArrayKey, Value)>,
    native: NativeDump<'_>,
) {
    out.extend_from_slice(display_class_name(d.layout().class_name()));
    // php heads an enum case with `Enum` (pure) or `Enum:int`/`Enum:string`
    // (backed) instead of `Object`, then lists its properties as usual.
    if d.flags().contains(rphp_value::ObjFlags::ENUM_CASE) {
        out.extend_from_slice(b" Enum");
        match d.props_in_order().find(|p| p.name == b"value").map(|p| p.value.clone()) {
            Some(rphp_value::Value::Int(_)) => out.extend_from_slice(b":int"),
            Some(rphp_value::Value::Str(_)) => out.extend_from_slice(b":string"),
            _ => {}
        }
        out.push(b'\n');
    } else {
        out.extend_from_slice(b" Object\n");
    }
    if seen.objects.contains(&d.id()) {
        out.extend_from_slice(b" *RECURSION*");
        return;
    }
    indent(out, pad);
    out.extend_from_slice(b"(\n");
    seen.objects.push(d.id());
    // An initialized lazy proxy lists the real instance under `[instance]`.
    if let Some(real) = d.lazy().and_then(|l| l.real.clone()) {
        indent(out, pad + 4);
        out.extend_from_slice(b"[instance] => ");
        print_r_buf(out, &Value::Object(real), pad + 8, seen, native);
        out.push(b'\n');
        seen.objects.pop();
        indent(out, pad);
        out.extend_from_slice(b")\n");
        return;
    }
    for (k, v) in &extra {
        indent(out, pad + 4);
        out.push(b'[');
        match k {
            ArrayKey::Int(i) => out.extend_from_slice(i.to_string().as_bytes()),
            ArrayKey::Str(s) => out.extend_from_slice(s),
        }
        out.extend_from_slice(b"] => ");
        print_r_buf(out, v, pad + 8, seen, native);
        out.push(b'\n');
    }
    for p in d.props_in_order().filter(|p| !p.value.is_uninit()) {
        indent(out, pad + 4);
        out.push(b'[');
        out.extend_from_slice(p.name);
        match p.vis {
            Vis::Public => {}
            Vis::Protected => out.extend_from_slice(b":protected"),
            Vis::Private => {
                out.push(b':');
                out.extend_from_slice(decl_class_name(&p));
                out.extend_from_slice(b":private");
            }
        }
        out.extend_from_slice(b"] => ");
        print_r_buf(out, p.value, pad + 8, seen, native);
        out.push(b'\n');
    }
    seen.objects.pop();
    indent(out, pad);
    out.extend_from_slice(b")\n");
}

#[cfg(test)]
mod tests {
    use std::rc::Rc;

    use rphp_value::{Layout, Object, PhpRef, PropMeta, Resource};

    use super::*;

    fn dumped(v: &Value) -> String {
        let mut out = Vec::new();
        dump(&mut out, v, 0, &mut Seen::new(), -1, &mut |_| None);
        String::from_utf8(out).unwrap()
    }

    fn printed(v: &Value) -> String {
        let mut out = Vec::new();
        print_r_buf(&mut out, v, 0, &mut Seen::new(), &mut |_| None);
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
