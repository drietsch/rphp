//! Output / debugging builtins: `var_dump`, `print_r`.
//!
//! These write to the run's stdout buffer ([`Ctx::out`]) rather than returning a
//! string (except `print_r($v, true)`, which returns it). The formatting is
//! byte-exact against stock PHP for scalars, arrays, objects (class name,
//! handle id, `:protected` / `:"Decl":private` annotations, `*RECURSION*`),
//! references (`&` on a shared element) and resources. Floats use a
//! shortest-round-trip form (PHP's `serialize_precision=-1`); the
//! scientific-notation threshold is a documented divergence axis until the
//! dedicated serializer lands. Closures keep a placeholder shape until they
//! become `Closure` objects (plan E6).
use rphp_value::{ArrayKey, ObjectData, PropEntry, Str, Value, Vis};

use crate::{nf, Ctx, NativeFn, NativeResult};

/// This extension's registry contribution (see `lib.rs`).
pub(crate) static FUNCTIONS: &[NativeFn] = &[
    nf!("var_dump", 1, None, var_dump),
    nf!("print_r", 1, Some(2), print_r),
];

/// Object ids currently being printed, to detect `$o->self = $o` cycles.
type Seen = Vec<u32>;

/// PHP `var_dump(...$values)`: dump each argument's type and value. Returns null.
pub(crate) fn var_dump(ctx: &mut Ctx, args: &[Value]) -> NativeResult {
    let mut seen = Seen::new();
    for v in args {
        dump(ctx.out(), v, 0, &mut seen);
    }
    Ok(Value::Null)
}

/// PHP `print_r($value, $return = false)`: human-readable form. With `$return`
/// truthy, returns the string; otherwise writes it to stdout and returns `true`.
pub(crate) fn print_r(ctx: &mut Ctx, args: &[Value]) -> NativeResult {
    let return_mode = args.get(1).is_some_and(Value::to_bool);
    let mut buf = Vec::new();
    print_r_buf(&mut buf, &args[0], 0, &mut Seen::new());
    if return_mode {
        Ok(Value::Str(Str::from_vec(buf)))
    } else {
        ctx.out().extend_from_slice(&buf);
        Ok(Value::Bool(true))
    }
}

// ---- var_dump ---------------------------------------------------------------

fn indent(out: &mut Vec<u8>, spaces: usize) {
    out.resize(out.len() + spaces, b' ');
}

fn dump_float(out: &mut Vec<u8>, f: f64) {
    let s = if f.is_nan() {
        "NAN".to_string()
    } else if f.is_infinite() {
        if f < 0.0 { "-INF" } else { "INF" }.to_string()
    } else {
        // Shortest round-trip, matching serialize_precision=-1 for the common
        // (non-scientific) magnitudes.
        format!("{f}")
    };
    out.extend_from_slice(s.as_bytes());
}

/// Emit the `var_dump` representation of `v`. `pad` is the indentation (in
/// spaces) of the *enclosing* container; the caller has already written `pad`
/// spaces before the value when this is an element.
fn dump(out: &mut Vec<u8>, v: &Value, pad: usize, seen: &mut Seen) {
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
            dump_float(out, *f);
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
                dump(out, val, pad + 2, seen);
            }
            indent(out, pad);
            out.extend_from_slice(b"}\n");
        }
        // A closure is an object; PHP prints `object(Closure)#N (3) { name, file,
        // line }` — the shape arrives with the Closure class (plan E6).
        Value::Closure(_) => out.extend_from_slice(b"object(Closure) {\n}\n"),
        Value::Object(o) => o.with_data(|d| dump_object(out, d, pad, seen)),
        // PHP marks a reference `&` only while more than one handle shares it.
        Value::Ref(r) => {
            if r.strong_count() > 1 {
                out.push(b'&');
            }
            dump(out, &r.borrow(), pad, seen);
        }
        Value::Resource(r) => {
            out.extend_from_slice(format!("resource({}) of type ({})\n", r.id(), r.kind()).as_bytes());
        }
    }
}

/// `object(Class)#id (count) { ["name"(:protected | :"Decl":private)]=> value … }`
fn dump_object(out: &mut Vec<u8>, d: &ObjectData, pad: usize, seen: &mut Seen) {
    if seen.contains(&d.id()) {
        out.extend_from_slice(b"*RECURSION*\n");
        return;
    }
    out.extend_from_slice(b"object(");
    out.extend_from_slice(d.layout().class_name());
    out.extend_from_slice(format!(")#{} ({}) {{\n", d.id(), d.prop_count()).as_bytes());
    seen.push(d.id());
    for p in d.props_in_order().filter(|p| !p.value.is_uninit()) {
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
        dump(out, p.value, pad + 2, seen);
    }
    seen.pop();
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
fn print_r_buf(out: &mut Vec<u8>, v: &Value, pad: usize, seen: &mut Seen) {
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
                print_r_buf(out, val, pad + 8, seen);
                out.push(b'\n');
            }
            indent(out, pad);
            out.extend_from_slice(b")\n");
        }
        Value::Object(o) => o.with_data(|d| print_r_object(out, d, pad, seen)),
        Value::Ref(r) => print_r_buf(out, &r.borrow(), pad, seen),
        // Scalars (and resources: `Resource id #N`) use the same string
        // conversion as `echo`.
        _ => v.append_php_bytes(out),
    }
}

/// `Class Object ( [name(:protected | :Decl:private)] => value … )`
fn print_r_object(out: &mut Vec<u8>, d: &ObjectData, pad: usize, seen: &mut Seen) {
    out.extend_from_slice(d.layout().class_name());
    out.extend_from_slice(b" Object\n");
    if seen.contains(&d.id()) {
        out.extend_from_slice(b" *RECURSION*");
        return;
    }
    indent(out, pad);
    out.extend_from_slice(b"(\n");
    seen.push(d.id());
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
        print_r_buf(out, p.value, pad + 8, seen);
        out.push(b'\n');
    }
    seen.pop();
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
        dump(&mut out, v, 0, &mut Seen::new());
        String::from_utf8(out).unwrap()
    }

    fn printed(v: &Value) -> String {
        let mut out = Vec::new();
        print_r_buf(&mut out, v, 0, &mut Seen::new());
        String::from_utf8(out).unwrap()
    }

    fn meta(name: &str, vis: Vis, decl: &str) -> PropMeta {
        PropMeta {
            name: Box::from(name.as_bytes()),
            vis,
            decl_class: 0,
            decl_class_name: Rc::from(decl.as_bytes()),
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
