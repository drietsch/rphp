//! php-src `ext/standard/var.c` + `var_unserializer.re`: `var_export`,
//! `serialize`, `unserialize`, and the `memory_get_usage` family.
//! (`var_dump`/`print_r` live in `output.rs`.)
//!
//! Floats use [`crate::output::php_gcvt`] — the
//! `serialize_precision=-1` layout `var_dump`/`var_export`/`serialize` share.
//! Objects serialize through their instance layout (visibility-mangled
//! names `\0*\0p` / `\0Decl\0p`); `unserialize` re-creates objects of classes
//! declared by the running program from the same layout the engine builds
//! for `new`. What is **not** modelled yet is called out loudly rather than
//! faked: `__sleep`/`__serialize`/`__wakeup`/`__unserialize` cannot be
//! invoked until magic methods land (E6) — a class declaring one makes the
//! call throw an `Error`; a class unknown to the program (`stdClass`,
//! `__PHP_Incomplete_Class`, enums, `C:` custom objects) makes `unserialize`
//! return `false` with php's `Error at offset` warning.


use rphp_runtime::{nf, Ctx, NativeFn, NativeResult, Registry, Unwind};
use rphp_value::{
    array_key, Array, ArrayKey, Object, PhpRef, Str, Value, Vis,
};

use crate::output::{php_gcvt, serialize_precision};

/// This extension's registry contribution (see `lib.rs`).
pub(crate) static FUNCTIONS: &[NativeFn] = &[
    nf!("var_export", 1, Some(2), var_export),
    nf!("serialize", 1, Some(1), serialize),
    nf!("unserialize", 1, Some(2), unserialize),
    nf!("memory_get_usage", 0, Some(1), memory_get_usage),
    nf!("memory_get_peak_usage", 0, Some(1), memory_get_peak_usage),
    nf!("memory_reset_peak_usage", 0, Some(0), memory_reset_peak_usage),
];

/// The ini directives `var.c` declares (`unserialize_max_depth`,
/// `unserialize_callback_func` is a core default already).
pub(crate) fn register_constants(r: &mut Registry) {
    r.interp().ini.register("unserialize_max_depth", "4096");
}

// ---- var_export -------------------------------------------------------------

/// `var_export(mixed $value, bool $return = false): ?string`
pub(crate) fn var_export(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let return_mode = args.get(1).is_some_and(Value::to_bool);
    let mut out = Vec::new();
    let mut seen = Seen::default();
    let precision = serialize_precision(ctx);
    export(ctx, &mut out, &args[0], 1, &mut seen, precision)?;
    if return_mode {
        Ok(Value::Str(Str::from_vec(out)))
    } else {
        ctx.out().extend_from_slice(&out);
        Ok(Value::Null)
    }
}

/// Containers on the current `var_export` path (objects by id, reference
/// cells by identity) — php's `GC_PROTECT_RECURSION`.
#[derive(Default)]
struct Seen {
    objects: Vec<u32>,
    refs: Vec<PhpRef>,
}

fn spaces(out: &mut Vec<u8>, n: usize) {
    out.resize(out.len() + n, b' ');
}

/// `php_addcslashes(s, "'\\")` — the quoting `var_export` applies to strings
/// and keys.
fn export_quote(out: &mut Vec<u8>, s: &[u8], nul_split: bool) {
    for &b in s {
        match b {
            b'\'' | b'\\' => {
                out.push(b'\\');
                out.push(b);
            }
            // A NUL byte cannot live in a single-quoted literal; php splices
            // in a double-quoted `"\0"`.
            0 if nul_split => out.extend_from_slice(b"' . \"\\0\" . '"),
            _ => out.push(b),
        }
    }
}

/// `php_var_export_ex(zv, level, buf)`. `level` starts at 1 for the outermost
/// value; nested containers open on a fresh line indented by `level - 1`.
fn export(ctx: &mut Ctx, out: &mut Vec<u8>, v: &Value, level: usize, seen: &mut Seen, precision: i64) -> Result<(), Unwind> {
    match v {
        Value::Null | Value::Uninit => out.extend_from_slice(b"NULL"),
        Value::Bool(true) => out.extend_from_slice(b"true"),
        Value::Bool(false) => out.extend_from_slice(b"false"),
        Value::Int(i) => {
            // `PHP_INT_MIN` has no literal; php prints it as an expression.
            if *i == i64::MIN {
                out.extend_from_slice(b"-9223372036854775807-1");
            } else {
                out.extend_from_slice(i.to_string().as_bytes());
            }
        }
        Value::Float(f) => {
            let s = php_gcvt(*f, precision, b'E');
            out.extend_from_slice(s.as_bytes());
            // `smart_str_append_double(.., zero_frac = true)`: an integral,
            // finite float keeps a `.0` so it re-reads as a float.
            if f.is_finite() && !s.contains(['.', 'E']) {
                out.extend_from_slice(b".0");
            }
        }
        Value::Str(s) => {
            out.push(b'\'');
            export_quote(out, s.as_bytes(), true);
            out.push(b'\'');
        }
        Value::Array(a) => {
            if level > 1 {
                out.push(b'\n');
                spaces(out, level - 1);
            }
            out.extend_from_slice(b"array (\n");
            for (k, val) in a.iter() {
                spaces(out, level + 1);
                match k {
                    ArrayKey::Int(i) => out.extend_from_slice(i.to_string().as_bytes()),
                    ArrayKey::Str(s) => {
                        out.push(b'\'');
                        export_quote(out, s, true);
                        out.push(b'\'');
                    }
                }
                out.extend_from_slice(b" => ");
                export(ctx, out, val, level + 2, seen, precision)?;
                out.extend_from_slice(b",\n");
            }
            if level > 1 {
                spaces(out, level - 1);
            }
            out.push(b')');
        }
        Value::Closure(_) => {
            // php: `\Closure::__set_state(array(\n))` — a closure exports no
            // properties.
            if level > 1 {
                out.push(b'\n');
                spaces(out, level - 1);
            }
            out.extend_from_slice(b"\\Closure::__set_state(array(\n");
            if level > 1 {
                spaces(out, level - 1);
            }
            out.extend_from_slice(b"))");
        }
        Value::Object(o) => {
            if seen.objects.contains(&o.id()) {
                ctx.warn("var_export does not handle circular references")?;
                out.extend_from_slice(b"NULL");
                return Ok(());
            }
            if level > 1 {
                out.push(b'\n');
                spaces(out, level - 1);
            }
            let class = o.layout().class_name().to_vec();
            // php exports an enum case as the case itself, `\Suit::Hearts`,
            // not a `__set_state` reconstruction.
            if o.flags().contains(rphp_value::ObjFlags::ENUM_CASE) {
                out.push(b'\\');
                out.extend_from_slice(&class);
                out.extend_from_slice(b"::");
                if let Some(v) = o.get_deref(b"name") {
                    out.extend_from_slice(&v.to_php_bytes());
                }
                return Ok(());
            }
            let std_class = class.eq_ignore_ascii_case(b"stdClass");
            if std_class {
                out.extend_from_slice(b"(object) array(\n");
            } else {
                out.push(b'\\');
                out.extend_from_slice(&class);
                out.extend_from_slice(b"::__set_state(array(\n");
            }
            seen.objects.push(o.id());
            // Copy the properties out before recursing: an element may hold a
            // handle back onto this object.
            let props = o.props_snapshot();
            for (name, val, _) in &props {
                if val.is_uninit() {
                    continue;
                }
                spaces(out, level + 2);
                out.push(b'\'');
                export_quote(out, name, false);
                out.extend_from_slice(b"' => ");
                export(ctx, out, val, level + 2, seen, precision)?;
                out.extend_from_slice(b",\n");
            }
            seen.objects.pop();
            if level > 1 {
                spaces(out, level - 1);
            }
            out.extend_from_slice(if std_class { b")" } else { b"))" });
        }
        Value::Ref(r) => {
            if seen.refs.iter().any(|s| s.ptr_eq(r)) {
                ctx.warn("var_export does not handle circular references")?;
                out.extend_from_slice(b"NULL");
                return Ok(());
            }
            seen.refs.push(r.clone());
            let inner = r.get();
            let res = export(ctx, out, &inner, level, seen, precision);
            seen.refs.pop();
            res?;
        }
        // php exports a resource as `NULL`.
        Value::Resource(_) => out.extend_from_slice(b"NULL"),
    }
    Ok(())
}

// ---- serialize --------------------------------------------------------------

/// `serialize(mixed $value): string`
pub(crate) fn serialize(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let mut out = Vec::new();
    let mut st = SerState { precision: serialize_precision(ctx), ..SerState::default() };
    ser(ctx, &mut out, &args[0], &mut st)?;
    Ok(Value::Str(Str::from_vec(out)))
}

/// php's `var_hash`: every value serialized gets a 1-based slot number;
/// objects and reference cells remember theirs so a second occurrence becomes
/// `r:N;` / `R:N;`.
#[derive(Default)]
struct SerState {
    precision: i64,
    n: i64,
    objects: Vec<(u32, i64)>,
    refs: Vec<(PhpRef, i64)>,
}

impl SerState {
    /// `php_add_var_hash`: bump the counter and return the earlier slot of an
    /// already-seen object / reference cell (0 = first sighting).
    fn add(&mut self, v: &Value) -> i64 {
        self.n += 1;
        match v {
            Value::Ref(r) => {
                let inner = r.borrow();
                if let Value::Object(o) = &*inner {
                    // A reference to an object is tracked as the object.
                    let id = o.id();
                    if let Some(&(_, n)) = self.objects.iter().find(|(i, _)| *i == id) {
                        self.n -= 1;
                        return n;
                    }
                    self.objects.push((id, self.n));
                    return 0;
                }
                drop(inner);
                if let Some((_, n)) = self.refs.iter().find(|(c, _)| c.ptr_eq(r)) {
                    // References are counted once: undo the increment.
                    self.n -= 1;
                    return *n;
                }
                self.refs.push((r.clone(), self.n));
                0
            }
            Value::Object(o) => {
                let id = o.id();
                if let Some(&(_, n)) = self.objects.iter().find(|(i, _)| *i == id) {
                    return n;
                }
                self.objects.push((id, self.n));
                0
            }
            _ => 0,
        }
    }
}

fn ser_len_str(out: &mut Vec<u8>, s: &[u8]) {
    out.extend_from_slice(b"s:");
    out.extend_from_slice(s.len().to_string().as_bytes());
    out.extend_from_slice(b":\"");
    out.extend_from_slice(s);
    out.extend_from_slice(b"\";");
}

/// The mangled property name php stores for a declared slot: `\0*\0p` for
/// protected, `\0Decl\0p` for private, the bare name otherwise.
fn mangled_name(name: &[u8], vis: Vis, decl: &[u8]) -> Vec<u8> {
    match vis {
        Vis::Public => name.to_vec(),
        Vis::Protected => {
            let mut v = Vec::with_capacity(name.len() + 3);
            v.extend_from_slice(b"\0*\0");
            v.extend_from_slice(name);
            v
        }
        Vis::Private => {
            let mut v = Vec::with_capacity(name.len() + decl.len() + 2);
            v.push(0);
            v.extend_from_slice(decl);
            v.push(0);
            v.extend_from_slice(name);
            v
        }
    }
}

/// Whether the running program declares (or inherits) `method` on `class`.
fn class_has_method(ctx: &Ctx, class: u32, method: &[u8]) -> bool {
    ctx.resolve_method(class, method).is_some()
}

/// `php_var_serialize_intern`: register the value in the var hash, then
/// serialize it (a reference is transparent — its target is written in
/// place without a second slot).
fn ser(ctx: &mut Ctx, out: &mut Vec<u8>, v: &Value, st: &mut SerState) -> Result<(), Unwind> {
    let seen = st.add(v);
    if seen != 0 {
        match v {
            Value::Ref(_) => {
                out.extend_from_slice(format!("R:{seen};").as_bytes());
                return Ok(());
            }
            Value::Object(_) => {
                out.extend_from_slice(format!("r:{seen};").as_bytes());
                return Ok(());
            }
            _ => {}
        }
    }
    let inner;
    let v = match v {
        Value::Ref(r) => {
            inner = r.get();
            &inner
        }
        v => v,
    };
    match v {
        Value::Null | Value::Uninit => out.extend_from_slice(b"N;"),
        Value::Bool(b) => out.extend_from_slice(if *b { b"b:1;" } else { b"b:0;" }),
        Value::Int(i) => out.extend_from_slice(format!("i:{i};").as_bytes()),
        Value::Float(f) => {
            out.extend_from_slice(b"d:");
            out.extend_from_slice(php_gcvt(*f, st.precision, b'E').as_bytes());
            out.push(b';');
        }
        Value::Str(s) => ser_len_str(out, s.as_bytes()),
        Value::Array(a) => {
            out.extend_from_slice(format!("a:{}:{{", a.len()).as_bytes());
            for (k, val) in a.iter() {
                match k {
                    ArrayKey::Int(i) => out.extend_from_slice(format!("i:{i};").as_bytes()),
                    ArrayKey::Str(s) => ser_len_str(out, s),
                }
                ser(ctx, out, val, st)?;
            }
            out.push(b'}');
        }
        Value::Object(o) => {
            let class = o.layout().class_name().to_vec();
            // An anonymous class cannot be named again on the way back in, so
            // php refuses it the way it refuses a closure.
            if class.contains(&0) {
                return Err(Unwind::exception(
                    "Exception",
                    format!(
                        "Serialization of '{}' is not allowed",
                        String::from_utf8_lossy(rphp_value::display_class_name(&class))
                    ),
                ));
            }
            // `__serialize()` replaces the property set outright: its array is
            // written as the object's payload, keys and all.
            if class_has_method(ctx, o.class_id(), b"__serialize") {
                let ret = ctx.call_method(o, b"__serialize", &[])?;
                let Value::Array(a) = &*ret.deref() else {
                    return Err(Unwind::type_error(format!(
                        "{}::__serialize() must return an array",
                        String::from_utf8_lossy(&class)
                    )));
                };
                let a = a.clone();
                out.extend_from_slice(format!("O:{}:\"", class.len()).as_bytes());
                out.extend_from_slice(&class);
                out.extend_from_slice(format!("\":{}:{{", a.len()).as_bytes());
                for (k, val) in a.iter() {
                    match k {
                        ArrayKey::Int(i) => out.extend_from_slice(format!("i:{i};").as_bytes()),
                        ArrayKey::Str(k) => ser_len_str(out, k),
                    }
                    ser(ctx, out, val, st)?;
                }
                out.push(b'}');
                return Ok(());
            }
            // Snapshot first: a property may hold a handle back onto `o`, and
            // the recursion must not hold the RefCell borrow.
            let mut props: Vec<(Vec<u8>, Vec<u8>, Value)> = o.with_data(|d| {
                d.props_in_order()
                    .filter(|p| !p.value.is_uninit())
                    .map(|p| {
                        let decl = p.meta.map_or(&b""[..], |m| &m.decl_class_name);
                        (p.name.to_vec(), mangled_name(p.name, p.vis, decl), p.value.clone())
                    })
                    .collect()
            });
            // `__sleep()` names the properties to keep, in its own order; php
            // warns about a name that is not a property and drops it.
            if class_has_method(ctx, o.class_id(), b"__sleep") {
                let ret = ctx.call_method(o, b"__sleep", &[])?;
                let Value::Array(names) = &*ret.deref() else {
                    ctx.warn(&format!(
                        "serialize(): {}::__sleep() should return an array only containing the names of instance-variables to serialize",
                        String::from_utf8_lossy(&class)
                    ))?;
                    out.extend_from_slice(b"N;");
                    return Ok(());
                };
                let names = names.clone();
                let mut kept: Vec<(Vec<u8>, Vec<u8>, Value)> = Vec::new();
                for (_, name) in names.iter() {
                    let name = name.deref().to_php_bytes().to_vec();
                    match props.iter().find(|(n, _, _)| *n == name) {
                        Some(p) => kept.push(p.clone()),
                        None => ctx.warn(&format!(
                            "serialize(): \"{}\" returned as member variable from __sleep() but does not exist",
                            String::from_utf8_lossy(&name)
                        ))?,
                    }
                }
                props = kept;
            }
            out.extend_from_slice(format!("O:{}:\"", class.len()).as_bytes());
            out.extend_from_slice(&class);
            out.extend_from_slice(format!("\":{}:{{", props.len()).as_bytes());
            for (_, name, val) in &props {
                ser_len_str(out, name);
                ser(ctx, out, val, st)?;
            }
            out.push(b'}');
        }
        Value::Closure(_) => {
            return Err(Unwind::exception("Exception", "Serialization of 'Closure' is not allowed"));
        }
        // php serializes a resource as `i:0;`.
        Value::Resource(_) => out.extend_from_slice(b"i:0;"),
        // Dereferenced above; a cell never holds another cell.
        Value::Ref(_) => out.extend_from_slice(b"N;"),
    }
    Ok(())
}

// ---- unserialize ------------------------------------------------------------

/// `unserialize(string $data, array $options = []): mixed`
pub(crate) fn unserialize(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let data = args[0].to_php_bytes();
    let mut max_depth = ctx.ini.int("unserialize_max_depth");
    let mut allowed = AllowedClasses::All;
    if let Some(opts) = args.get(1) {
        let opts = opts.deref().into_owned();
        let Value::Array(opts) = opts else {
            return Err(Unwind::type_error(format!(
                "unserialize(): Argument #2 ($options) must be of type array, {} given",
                opts.type_name()
            )));
        };
        if let Some(v) = opts.get_deref(&ArrayKey::str(b"allowed_classes")) {
            allowed = match v {
                Value::Bool(true) => AllowedClasses::All,
                Value::Bool(false) => AllowedClasses::None,
                Value::Array(a) => AllowedClasses::Some(a.iter().map(|(_, v)| v.to_php_bytes()).collect()),
                other => {
                    return Err(Unwind::type_error(format!(
                        "unserialize(): Option \"allowed_classes\" must be of type array|bool, {} given",
                        other.type_name()
                    )))
                }
            };
        }
        if let Some(v) = opts.get_deref(&ArrayKey::str(b"max_depth")) {
            let Value::Int(d) = v else {
                return Err(Unwind::type_error(format!(
                    "unserialize(): Option \"max_depth\" must be of type int, {} given",
                    v.type_name()
                )));
            };
            if d < 0 {
                return Err(Unwind::value_error(
                    "unserialize(): Option \"max_depth\" must be greater than or equal to 0",
                ));
            }
            max_depth = d;
        }
    }
    if data.is_empty() {
        return Ok(Value::Bool(false));
    }
    let mut p = Unserializer {
        data: &data,
        chain: Vec::new(),
        slots: Vec::new(),
        depth: 0,
        max_depth,
        allowed,
    };
    match p.value(ctx, 0, true) {
        Ok((v, end)) => {
            // A root that references itself comes back in its cell; hand out
            // the value (its elements keep the cell alive).
            let v = match v {
                Value::Ref(r) => r.get(),
                v => v,
            };
            if end < data.len() {
                ctx.warn(&format!(
                    "unserialize(): Extra data starting at offset {end} of {} bytes",
                    data.len()
                ))?;
            }
            Ok(v)
        }
        Err(UErr::At(off)) => {
            ctx.warn(&format!("unserialize(): Error at offset {off} of {} bytes", data.len()))?;
            Ok(Value::Bool(false))
        }
        Err(UErr::Unwind(u)) => Err(u),
    }
}

/// The `allowed_classes` option.
enum AllowedClasses {
    All,
    None,
    Some(Vec<Vec<u8>>),
}

impl AllowedClasses {
    fn allows(&self, class: &[u8]) -> bool {
        match self {
            AllowedClasses::All => true,
            AllowedClasses::None => false,
            AllowedClasses::Some(list) => list.iter().any(|c| c.eq_ignore_ascii_case(class)),
        }
    }
}

/// A parse failure: php's `Error at offset N` (the position `*p` was left
/// at), or a diagnostic that unwound (a warning turned into an exception by
/// the user's error handler).
enum UErr {
    At(usize),
    Unwind(Unwind),
}

impl From<Unwind> for UErr {
    fn from(u: Unwind) -> UErr {
        UErr::Unwind(u)
    }
}

/// One step from a container to an element: the key an array element lives
/// under, or a property name.
#[derive(Clone, PartialEq, Eq)]
enum Step {
    Elem(ArrayKey),
    Prop(Vec<u8>),
}

/// A container whose elements are still being parsed.
enum Pending {
    Arr(Array),
    Obj(Object),
}

/// An open container on the parse path, with the step from its parent (none
/// for the root value). `cell` is created when an `R:` inside the container
/// points back at the container itself (`$a[0] = &$a`): the finished value
/// then lives in that cell and the parent holds a reference to it.
struct Open {
    container: Pending,
    step: Option<Step>,
    cell: Option<PhpRef>,
}

impl Open {
    fn new(container: Pending, step: Option<Step>) -> Open {
        Open { container, step, cell: None }
    }
}

/// A finished container: wrapped in the shared cell when one was handed out.
fn finish_value(cell: Option<PhpRef>, value: Value) -> Value {
    match cell {
        Some(cell) => {
            cell.set(value);
            Value::Ref(cell)
        }
        None => value,
    }
}

/// php's `var_hash` slot: where the value landed (a path from the root
/// through the containers) and, for `r:N;`, the object handle.
struct Slot {
    path: Vec<Step>,
    object: Option<Object>,
}

struct Unserializer<'a> {
    data: &'a [u8],
    chain: Vec<Open>,
    slots: Vec<Slot>,
    depth: i64,
    max_depth: i64,
    allowed: AllowedClasses,
}

impl<'a> Unserializer<'a> {
    /// The byte at `i`, or NUL past the end (php scans a NUL-terminated
    /// buffer).
    fn at(&self, i: usize) -> u8 {
        self.data.get(i).copied().unwrap_or(0)
    }

    /// The path a value parsed under `key` in the innermost open container
    /// gets.
    fn path_for(&self, key: Option<Step>) -> Vec<Step> {
        let mut path: Vec<Step> = self.chain.iter().filter_map(|o| o.step.clone()).collect();
        if let Some(k) = key {
            path.push(k);
        }
        path
    }

    /// `parse_iv2`: an optionally signed decimal run at `i`; returns the value
    /// and the position after it. Overflow saturates with php's warning.
    fn parse_iv(&self, ctx: &mut Ctx, mut i: usize) -> Result<(i64, usize), Unwind> {
        let mut neg = false;
        if self.at(i) == b'-' {
            neg = true;
            i += 1;
        } else if self.at(i) == b'+' {
            i += 1;
        }
        while self.at(i) == b'0' {
            i += 1;
        }
        let start = i;
        let mut acc: u64 = 0;
        let mut overflow = false;
        while self.at(i).is_ascii_digit() {
            acc = match acc.checked_mul(10).and_then(|a| a.checked_add((self.at(i) - b'0') as u64)) {
                Some(a) => a,
                None => {
                    overflow = true;
                    acc
                }
            };
            i += 1;
        }
        if overflow || i - start > 19 || acc > i64::MAX as u64 + neg as u64 {
            ctx.warn("unserialize(): Numerical result out of range")?;
            return Ok((if neg { i64::MIN } else { i64::MAX }, i));
        }
        let v = if neg { (acc as i64).wrapping_neg() } else { acc as i64 };
        Ok((v, i))
    }

    /// `[0-9]+` at `i` (a length / slot number), with its end.
    fn scan_uiv(&self, i: usize) -> Option<(usize, usize)> {
        let mut j = i;
        let mut acc: usize = 0;
        while self.at(j).is_ascii_digit() {
            acc = acc.wrapping_mul(10).wrapping_add((self.at(j) - b'0') as usize);
            j += 1;
        }
        (j > i).then_some((acc, j))
    }

    /// Match `[+-]? [0-9]+` at `i`.
    fn scan_iv(&self, i: usize) -> Option<usize> {
        let mut j = i;
        if matches!(self.at(j), b'+' | b'-') {
            j += 1;
        }
        let d = j;
        while self.at(j).is_ascii_digit() {
            j += 1;
        }
        (j > d).then_some(j)
    }

    /// Match the `d:` number grammar `iv | nv | nvexp` at `i`.
    fn scan_number(&self, i: usize) -> Option<usize> {
        let mut j = i;
        if matches!(self.at(j), b'+' | b'-') {
            j += 1;
        }
        let int_start = j;
        while self.at(j).is_ascii_digit() {
            j += 1;
        }
        let int_digits = j - int_start;
        let mut frac_digits = 0;
        if self.at(j) == b'.' {
            let k = j + 1;
            let mut m = k;
            while self.at(m).is_ascii_digit() {
                m += 1;
            }
            frac_digits = m - k;
            if int_digits == 0 && frac_digits == 0 {
                return None;
            }
            j = m;
        } else if int_digits == 0 {
            return None;
        }
        let _ = frac_digits;
        if matches!(self.at(j), b'e' | b'E') {
            if let Some(k) = self.scan_iv(j + 1) {
                j = k;
            }
        }
        Some(j)
    }

    /// `php_var_unserialize_internal`: parse one value at `start`. `var_hash`
    /// is false for array keys (no slot, no nesting, no references). Returns
    /// the value and the position after it.
    fn value(&mut self, ctx: &mut Ctx, start: usize, var_hash: bool) -> Result<(Value, usize), UErr> {
        self.value_in(ctx, start, var_hash, None)
    }

    fn value_in(
        &mut self,
        ctx: &mut Ctx,
        start: usize,
        var_hash: bool,
        key: Option<Step>,
    ) -> Result<(Value, usize), UErr> {
        if start >= self.data.len() {
            return Err(UErr::At(start));
        }
        let tag = self.at(start);
        let slot = if var_hash && tag != b'R' {
            self.slots.push(Slot { path: self.path_for(key.clone()), object: None });
            Some(self.slots.len() - 1)
        } else {
            None
        };
        let colon = self.at(start + 1) == b':';
        match tag {
            b'N' if self.at(start + 1) == b';' => Ok((Value::Null, start + 2)),
            b'b' if colon && self.at(start + 3) == b';' && matches!(self.at(start + 2), b'0' | b'1') => {
                Ok((Value::Bool(self.at(start + 2) == b'1'), start + 4))
            }
            b'i' if colon => {
                let Some(end) = self.scan_iv(start + 2) else {
                    return Err(UErr::At(start));
                };
                if self.at(end) != b';' {
                    return Err(UErr::At(start));
                }
                let (v, _) = self.parse_iv(ctx, start + 2)?;
                Ok((Value::Int(v), end + 1))
            }
            b'd' if colon => {
                let body = &self.data[start + 2..];
                for (lit, v) in [(&b"NAN;"[..], f64::NAN), (b"INF;", f64::INFINITY), (b"-INF;", f64::NEG_INFINITY)] {
                    if body.starts_with(lit) {
                        return Ok((Value::Float(v), start + 2 + lit.len()));
                    }
                }
                let Some(end) = self.scan_number(start + 2) else {
                    return Err(UErr::At(start));
                };
                if self.at(end) != b';' {
                    return Err(UErr::At(start));
                }
                let text = std::str::from_utf8(&self.data[start + 2..end]).unwrap_or("0");
                let f = parse_strtod(text);
                Ok((Value::Float(f), end + 1))
            }
            b's' | b'S' if colon => {
                let Some((len, after)) = self.scan_uiv(start + 2) else {
                    return Err(UErr::At(start));
                };
                if self.at(after) != b':' || self.at(after + 1) != b'"' {
                    return Err(UErr::At(start));
                }
                let mut cur = after + 2;
                let maxlen = self.data.len().saturating_sub(cur);
                if maxlen < len {
                    return Err(UErr::At(start + 2));
                }
                let bytes = if tag == b's' {
                    let b = self.data[cur..cur + len].to_vec();
                    cur += len;
                    b
                } else {
                    // `S:` — `\xx` hex escapes, each counting as one byte of
                    // the declared length.
                    let mut b = Vec::with_capacity(len);
                    for _ in 0..len {
                        if cur >= self.data.len() {
                            return Err(UErr::At(start));
                        }
                        if self.at(cur) != b'\\' {
                            b.push(self.at(cur));
                        } else {
                            let hi = hex_digit(self.at(cur + 1));
                            let lo = hex_digit(self.at(cur + 2));
                            match (hi, lo) {
                                (Some(h), Some(l)) => b.push((h << 4) | l),
                                _ => return Err(UErr::At(start)),
                            }
                            cur += 2;
                        }
                        cur += 1;
                    }
                    b
                };
                if self.at(cur) != b'"' {
                    return Err(UErr::At(cur));
                }
                if self.at(cur + 1) != b';' {
                    return Err(UErr::At(cur + 1));
                }
                if tag == b'S' {
                    ctx.deprecated("unserialize(): Unserializing the 'S' format is deprecated")?;
                }
                Ok((Value::Str(Str::from_vec(bytes)), cur + 2))
            }
            b'a' if colon => {
                let Some((_, after)) = self.scan_uiv(start + 2) else {
                    return Err(UErr::At(start));
                };
                if self.at(after) != b':' || self.at(after + 1) != b'{' {
                    return Err(UErr::At(start));
                }
                let (elements, _) = self.parse_iv(ctx, start + 2)?;
                let cur = after + 2;
                if !var_hash {
                    return Err(UErr::At(cur));
                }
                if elements < 0 || elements as usize > self.data.len() - cur {
                    return Err(UErr::At(cur));
                }
                self.chain.push(Open::new(Pending::Arr(Array::new()), key));
                let r = self.array_elements(ctx, cur, elements as usize);
                let open = self.chain.pop().expect("pushed above");
                let cur = r?;
                let cur = self.finish_nested(cur)?;
                let Open { container: Pending::Arr(a), cell, .. } = open else { unreachable!() };
                Ok((finish_value(cell, Value::Array(a)), cur))
            }
            b'O' if colon => {
                let Some((len, after)) = self.scan_uiv(start + 2) else {
                    return Err(UErr::At(start));
                };
                if self.at(after) != b':' || self.at(after + 1) != b'"' {
                    return Err(UErr::At(start));
                }
                let mut cur = after + 2;
                let maxlen = self.data.len().saturating_sub(cur);
                if maxlen < len || len == 0 {
                    return Err(UErr::At(start + 2));
                }
                let class = self.data[cur..cur + len].to_vec();
                cur += len;
                if self.at(cur) != b'"' {
                    return Err(UErr::At(cur));
                }
                if self.at(cur + 1) != b':' {
                    return Err(UErr::At(cur + 1));
                }
                if !var_hash {
                    return Err(UErr::At(cur));
                }
                if cur + 2 >= self.data.len() {
                    ctx.warn("Bad unserialize data")?;
                    return Err(UErr::At(cur));
                }
                let (elements, after_count) = self.parse_iv(ctx, cur + 2)?;
                if elements < 0 || elements as usize > self.data.len().saturating_sub(after_count) {
                    return Err(UErr::At(after_count));
                }
                if self.at(after_count) != b':' {
                    return Err(UErr::At(after_count));
                }
                if self.at(after_count + 1) != b'{' {
                    return Err(UErr::At(after_count + 1));
                }
                let cur = after_count + 2;
                // Only classes the running program declares can be
                // instantiated; `stdClass`, `__PHP_Incomplete_Class` and
                // disallowed classes land with the class model (E6).
                // A class the program does not declare — or one the
                // `allowed_classes` option rules out — becomes php's
                // `__PHP_Incomplete_Class`, whose first property is the name
                // that could not be resolved.
                let allowed = self.allowed.allows(&class);
                let resolved = if allowed { ctx.class_by_name(&class) } else { None };
                let incomplete = resolved.is_none();
                let class_id = match resolved {
                    Some(id) => id,
                    None => match ctx.class_by_name(b"__PHP_Incomplete_Class") {
                        Some(id) => id,
                        None => return Err(UErr::At(start)),
                    },
                };
                // Build the instance the way the engine's `new` does: the
                // class's parent-first property set with its defaults, under
                // the next object id. The constructor never runs.
                let obj = ctx.instantiate(class_id);
                if incomplete {
                    obj.set(b"__PHP_Incomplete_Class_Name", Value::string(&class));
                }
                if let Some(s) = slot {
                    self.slots[s].object = Some(obj.clone());
                }
                // `__unserialize()` owns the payload: php reads it as a plain
                // array, hands it over and leaves the properties alone (and
                // `__wakeup()` is then not called).
                if ctx.resolve_method(class_id, b"__unserialize").is_some() {
                    self.chain.push(Open::new(Pending::Arr(Array::new()), key));
                    let r = self.array_elements(ctx, cur, elements as usize);
                    let open = self.chain.pop().expect("pushed above");
                    let cur = r?;
                    let cur = self.finish_nested(cur)?;
                    let Open { container: Pending::Arr(a), cell, .. } = open else { unreachable!() };
                    ctx.call_method(&obj, b"__unserialize", &[Value::Array(a)])
                        .map_err(UErr::Unwind)?;
                    return Ok((finish_value(cell, Value::Object(obj)), cur));
                }
                self.chain.push(Open::new(Pending::Obj(obj.clone()), key));
                let r = self.object_props(ctx, cur, elements as usize, &obj);
                let open = self.chain.pop().expect("pushed above");
                let cur = r?;
                let cur = self.finish_nested(cur)?;
                // php wakes each object as soon as its own data is restored,
                // so a nested object wakes before the one holding it.
                if ctx.resolve_method(class_id, b"__wakeup").is_some() {
                    ctx.call_method(&obj, b"__wakeup", &[]).map_err(UErr::Unwind)?;
                }
                Ok((finish_value(open.cell, Value::Object(obj)), cur))
            }
            b'r' | b'R' if colon => {
                let Some((id, after)) = self.scan_uiv(start + 2) else {
                    return Err(UErr::At(start));
                };
                if self.at(after) != b';' {
                    return Err(UErr::At(start));
                }
                let cur = after + 1;
                if !var_hash || id == 0 || id > self.slots.len() {
                    return Err(UErr::At(cur));
                }
                let target = id - 1;
                if tag == b'r' {
                    // `r:` only ever points at an object.
                    let Some(o) = self.slots[target].object.clone() else {
                        return Err(UErr::At(cur));
                    };
                    if let Some(s) = slot {
                        self.slots[s].object = Some(o.clone());
                    }
                    return Ok((Value::Object(o), cur));
                }
                let path = self.slots[target].path.clone();
                // A slot that is the element being written (a repeated key
                // pointing at itself) is rejected, as php's `rval_ref == rval`.
                if path == self.path_for(key) {
                    return Err(UErr::At(cur));
                }
                let Some(cell) = self.reference_to(&path) else {
                    return Err(UErr::At(cur));
                };
                Ok((Value::Ref(cell), cur))
            }
            b'C' if colon => {
                // `Serializable` custom data: `C:len:"Class":datalen:{...}`.
                let Some((len, after)) = self.scan_uiv(start + 2) else {
                    return Err(UErr::At(start));
                };
                if self.at(after) != b':' || self.at(after + 1) != b'"' {
                    return Err(UErr::At(start));
                }
                let mut cur = after + 2;
                let maxlen = self.data.len().saturating_sub(cur);
                if maxlen < len || len == 0 {
                    return Err(UErr::At(start + 2));
                }
                let class = self.data[cur..cur + len].to_vec();
                cur += len;
                if self.at(cur) != b'"' {
                    return Err(UErr::At(cur));
                }
                if self.at(cur + 1) != b':' {
                    return Err(UErr::At(cur + 1));
                }
                if !var_hash {
                    return Err(UErr::At(cur));
                }
                // `object_custom`: the data length, then the data after `:{`.
                let (datalen, after_len) = self.parse_iv(ctx, cur + 2)?;
                let cur = after_len + 2;
                let present = self.data.len().saturating_sub(cur);
                if datalen < 0 || present <= datalen as usize {
                    ctx.warn(&format!(
                        "Insufficient data for unserializing - {datalen} required, {present} present"
                    ))?;
                    return Err(UErr::At(cur));
                }
                let datalen = datalen as usize;
                if self.at(cur + datalen) != b'}' {
                    return Err(UErr::At(cur + datalen));
                }
                // No class of the running program implements `Serializable`
                // (interfaces are not lowered yet): a declared class gets
                // php's "no unserializer" warning and a bare instance;
                // anything else needs `__PHP_Incomplete_Class` (E6).
                let class_id = if self.allowed.allows(&class) { ctx.class_by_name(&class) } else { None };
                let Some(class_id) = class_id else {
                    return Err(UErr::At(start));
                };
                let class_name = String::from_utf8_lossy(&ctx.class(class_id).name).into_owned();
                ctx.warn(&format!("Class {class_name} has no unserializer"))?;
                let obj = ctx.instantiate(class_id);
                if let Some(s) = slot {
                    self.slots[s].object = Some(obj.clone());
                }
                Ok((Value::Object(obj), cur + datalen + 1))
            }
            b'E' if colon => {
                // Enum cases: `E:len:"Class:Case";` — no enum exists until
                // the class model lands, so this always fails with php's
                // diagnostic for the class.
                let Some((len, after)) = self.scan_uiv(start + 2) else {
                    return Err(UErr::At(start));
                };
                if self.at(after) != b':' || self.at(after + 1) != b'"' {
                    return Err(UErr::At(start));
                }
                let cur = after + 2;
                if self.data.len().saturating_sub(cur) < len || len == 0 {
                    return Err(UErr::At(start + 2));
                }
                let name = self.data[cur..cur + len].to_vec();
                let text = String::from_utf8_lossy(&name).into_owned();
                match name.iter().position(|&b| b == b':') {
                    None => ctx.warn(&format!("unserialize(): Invalid enum name '{text}' (missing colon)"))?,
                    Some(colon_at) => {
                        let class = &name[..colon_at];
                        let known = ctx.class_exists(class);
                        let class = String::from_utf8_lossy(class);
                        if known {
                            ctx.warn(&format!("unserialize(): Class '{class}' is not an enum"))?;
                        } else {
                            ctx.warn(&format!("unserialize(): Class '{class}' not found"))?;
                        }
                    }
                }
                Err(UErr::At(start))
            }
            b'}' => {
                ctx.warn("unserialize(): Unexpected end of serialized data")?;
                Err(UErr::At(start))
            }
            _ => Err(UErr::At(start)),
        }
    }

    /// `finish_nested_data`: the closing `}` of an array / object body.
    fn finish_nested(&self, cur: usize) -> Result<usize, UErr> {
        if cur >= self.data.len() || self.at(cur) != b'}' {
            return Err(UErr::At(cur));
        }
        Ok(cur + 1)
    }

    /// The depth check both nested-data loops perform on entry.
    fn enter_nested(&mut self, ctx: &mut Ctx, cur: usize) -> Result<(), UErr> {
        if self.max_depth > 0 && self.depth >= self.max_depth {
            ctx.warn(&format!(
                "unserialize(): Maximum depth of {} exceeded. The depth limit can be changed using the max_depth unserialize() option or the unserialize_max_depth ini setting",
                self.max_depth
            ))?;
            return Err(UErr::At(cur));
        }
        self.depth += 1;
        Ok(())
    }

    /// `process_nested_array_data`: `elements` key/value pairs into the
    /// innermost open array.
    fn array_elements(&mut self, ctx: &mut Ctx, mut cur: usize, elements: usize) -> Result<usize, UErr> {
        self.enter_nested(ctx, cur)?;
        let r = (|| {
            for _ in 0..elements {
                let (key, after) = self.value(ctx, cur, false)?;
                let key = match key {
                    Value::Int(i) => ArrayKey::Int(i),
                    Value::Str(s) => array_key(&Value::Str(s)).expect("a string is a valid key"),
                    _ => return Err(UErr::At(after)),
                };
                // The slot exists (as null) before its value is parsed, so a
                // nested `R:` can already reach it; a repeated key replaces
                // the earlier slot outright (a reference binding is dropped,
                // not written through).
                if let Some(Open { container: Pending::Arr(a), .. }) = self.chain.last_mut() {
                    match a.get_mut(&key) {
                        Some(slot) => *slot = Value::Null,
                        None => a.set(key.clone(), Value::Null),
                    }
                }
                let (val, after) = self.value_in(ctx, after, true, Some(Step::Elem(key.clone())))?;
                if let Some(Open { container: Pending::Arr(a), .. }) = self.chain.last_mut() {
                    match a.get_mut(&key) {
                        Some(slot) => *slot = val,
                        None => a.set(key, val),
                    }
                }
                cur = after;
            }
            Ok(cur)
        })();
        self.depth -= 1;
        r
    }

    /// `process_nested_object_data`: `elements` name/value pairs onto `obj`.
    fn object_props(&mut self, ctx: &mut Ctx, mut cur: usize, elements: usize, obj: &Object) -> Result<usize, UErr> {
        self.enter_nested(ctx, cur)?;
        let r = (|| {
            for _ in 0..elements {
                let (key, after) = self.value(ctx, cur, false)?;
                let mangled = match key {
                    Value::Int(i) => i.to_string().into_bytes(),
                    Value::Str(s) => s.as_bytes().to_vec(),
                    _ => return Err(UErr::At(after)),
                };
                let name = unmangle(&mangled).to_vec();
                let declared = obj.layout().slot_of(&name).is_some();
                if !declared && obj.get(&name).is_none() {
                    let class = obj.layout().class_name().to_vec();
                    // Any class php lets grow properties silently —
                    // `stdClass`, `__PHP_Incomplete_Class`, one marked
                    // `#[AllowDynamicProperties]`.
                    let allows = ctx.class(obj.class_id()).allows_dynamic_props();
                    if !allows {
                        ctx.deprecated(&format!(
                            "Creation of dynamic property {}::${} is deprecated",
                            String::from_utf8_lossy(&class),
                            String::from_utf8_lossy(&name)
                        ))?;
                    }
                    obj.set(&name, Value::Null);
                } else {
                    // A repeated name replaces the slot (dropping a reference
                    // binding) rather than writing through it.
                    obj.with_data_mut(|d| {
                        if let Some(slot) = d.get_mut(&name) {
                            *slot = Value::Null;
                        }
                    });
                }
                let (val, after) = self.value_in(ctx, after, true, Some(Step::Prop(name.clone())))?;
                obj.with_data_mut(|d| {
                    if let Some(slot) = d.get_mut(&name) {
                        *slot = val;
                    }
                });
                cur = after;
            }
            Ok(cur)
        })();
        self.depth -= 1;
        r
    }

    /// Turn the value at `path` into a reference cell shared with the new
    /// `R:` element. A target that is a container still being parsed (an
    /// ancestor — `$a[0] = &$a`) gets a cell now that receives the finished
    /// container when it closes.
    fn reference_to(&mut self, path: &[Step]) -> Option<PhpRef> {
        let open_steps: Vec<Step> = self.chain.iter().filter_map(|o| o.step.clone()).collect();
        let mut i = 0;
        while i < path.len() && i < open_steps.len() && path[i] == open_steps[i] {
            i += 1;
        }
        if i >= path.len() {
            let open = self.chain.get_mut(i)?;
            return Some(open.cell.get_or_insert_with(|| PhpRef::new(Value::Null)).clone());
        }
        let open = self.chain.get_mut(i)?;
        match &mut open.container {
            Pending::Arr(a) => {
                let Step::Elem(k) = &path[i] else { return None };
                let slot = a.get_mut(k)?;
                ref_at(slot, &path[i + 1..])
            }
            Pending::Obj(o) => {
                let Step::Prop(n) = &path[i] else { return None };
                o.with_data_mut(|d| {
                    let slot = d.get_mut(n)?;
                    ref_at(slot, &path[i + 1..])
                })
            }
        }
    }
}

/// Descend `steps` from `v` and bind the element reached as a reference cell.
fn ref_at(v: &mut Value, steps: &[Step]) -> Option<PhpRef> {
    let Some((first, rest)) = steps.split_first() else {
        return Some(Value::make_ref(v));
    };
    match v {
        Value::Array(a) => {
            let Step::Elem(k) = first else { return None };
            ref_at(a.get_mut(k)?, rest)
        }
        Value::Object(o) => {
            let Step::Prop(n) = first else { return None };
            o.with_data_mut(|d| ref_at(d.get_mut(n)?, rest))
        }
        Value::Ref(r) => r.update(|inner| ref_at(inner, steps)),
        _ => None,
    }
}

fn hex_digit(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// `zend_strtod` on the matched number text (which may end in a bare `.` or
/// an exponent without digits — both parse as php does).
fn parse_strtod(text: &str) -> f64 {
    let t = text.strip_suffix('.').unwrap_or(text);
    let t = t.strip_prefix('+').unwrap_or(t);
    let t = if t.starts_with('.') {
        format!("0{t}")
    } else if let Some(rest) = t.strip_prefix("-.") {
        format!("-0.{rest}")
    } else {
        t.to_string()
    };
    match t.parse::<f64>() {
        Ok(f) => f,
        Err(_) => {
            // `1e` / `1e+`: the exponent part did not match; strtod stops
            // before the `e`.
            let cut = t.find(['e', 'E']).unwrap_or(t.len());
            t[..cut].parse::<f64>().unwrap_or(0.0)
        }
    }
}

/// `zend_unmangle_property_name`: strip the `\0*\0` / `\0Class\0` visibility
/// prefix php stores on protected / private properties.
fn unmangle(mangled: &[u8]) -> &[u8] {
    if mangled.first() == Some(&0) {
        if let Some(rest) = mangled[1..].iter().position(|&b| b == 0) {
            return &mangled[rest + 2..];
        }
    }
    mangled
}

// ---- memory -----------------------------------------------------------------

/// The engine has no allocator accounting yet; these are php's typical `-n`
/// CLI figures so callers computing deltas see a stable, plausible baseline
/// (allowlisted as `platform-value`).
const MEMORY_USAGE: i64 = 393_216;
const MEMORY_REAL_USAGE: i64 = 2_097_152;

/// `memory_get_usage(bool $real_usage = false): int`
pub(crate) fn memory_get_usage(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let real = args.first().is_some_and(Value::to_bool);
    Ok(Value::Int(if real { MEMORY_REAL_USAGE } else { MEMORY_USAGE }))
}

/// `memory_get_peak_usage(bool $real_usage = false): int`
pub(crate) fn memory_get_peak_usage(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    memory_get_usage(ctx, args)
}

/// `memory_reset_peak_usage(): void`
pub(crate) fn memory_reset_peak_usage(_: &mut Ctx, _: &mut [Value]) -> NativeResult {
    Ok(Value::Null)
}

#[cfg(test)]
mod tests {
    use rphp_runtime::ErrorKind;
    use rphp_value::{Array, ArrayKey, Value};

    use crate::tests::{arr, call_named, interp};

    fn s(b: &str) -> Value {
        Value::string(b.as_bytes())
    }

    fn export(v: Value) -> String {
        String::from_utf8(call_named(b"var_export", &[v, Value::Bool(true)]).to_php_bytes()).unwrap()
    }

    fn ser(v: Value) -> String {
        String::from_utf8(call_named(b"serialize", &[v]).to_php_bytes()).unwrap()
    }

    #[test]
    fn var_export_scalars_match_php() {
        assert_eq!(export(Value::Null), "NULL");
        assert_eq!(export(Value::Bool(true)), "true");
        assert_eq!(export(Value::Int(i64::MIN)), "-9223372036854775807-1");
        assert_eq!(export(Value::Float(1.0)), "1.0");
        assert_eq!(export(Value::Float(-0.0)), "-0.0");
        assert_eq!(export(Value::Float(1e25)), "1.0E+25");
        assert_eq!(export(Value::Float(1e15)), "1000000000000000.0");
        assert_eq!(export(Value::Float(f64::INFINITY)), "INF");
        assert_eq!(export(s("it's")), "'it\\'s'");
        assert_eq!(export(s("nul\0byte")), "'nul' . \"\\0\" . 'byte'");
        assert_eq!(export(s("\0")), "'' . \"\\0\" . ''");
    }

    #[test]
    fn var_export_arrays_nest_like_php() {
        assert_eq!(export(Value::empty_array()), "array (\n)");
        let mut a = Array::new();
        a.set(ArrayKey::str(b"a"), Value::Int(1));
        a.set(ArrayKey::str(b"b"), arr(&[Value::Int(1), Value::Int(2)]));
        a.set(ArrayKey::Int(5), s("x"));
        assert_eq!(
            export(Value::Array(a)),
            "array (\n  'a' => 1,\n  'b' => \n  array (\n    0 => 1,\n    1 => 2,\n  ),\n  5 => 'x',\n)"
        );
    }

    #[test]
    fn serialize_scalars_and_arrays_match_php() {
        assert_eq!(ser(Value::Null), "N;");
        assert_eq!(ser(Value::Bool(false)), "b:0;");
        assert_eq!(ser(Value::Int(-7)), "i:-7;");
        assert_eq!(ser(Value::Float(1.0)), "d:1;");
        assert_eq!(ser(Value::Float(0.1)), "d:0.1;");
        assert_eq!(ser(Value::Float(1e25)), "d:1.0E+25;");
        assert_eq!(ser(Value::Float(f64::NAN)), "d:NAN;");
        assert_eq!(ser(s("a\0b")), "s:3:\"a\0b\";");
        let mut a = Array::new();
        a.push(Value::Int(1));
        a.set(ArrayKey::str(b"k"), arr(&[Value::Bool(true), Value::Null]));
        assert_eq!(ser(Value::Array(a)), "a:2:{i:0;i:1;s:1:\"k\";a:2:{i:0;b:1;i:1;N;}}");
    }

    #[test]
    fn serialize_marks_shared_references() {
        // $a = [1]; $a[] = &$a[0];  =>  a:2:{i:0;i:1;i:1;R:2;}
        let mut a = Array::new();
        a.push(Value::Int(1));
        let r = a.get_ref(ArrayKey::Int(0));
        a.push_ref(r);
        assert_eq!(ser(Value::Array(a)), "a:2:{i:0;i:1;i:1;R:2;}");
    }

    #[test]
    fn unserialize_round_trips_and_reports_offsets() {
        let un = |text: &str| call_named(b"unserialize", &[s(text)]);
        assert_eq!(un("N;"), Value::Null);
        assert_eq!(un("b:1;"), Value::Bool(true));
        assert_eq!(un("i:-42;"), Value::Int(-42));
        assert_eq!(un("d:0.5;"), Value::Float(0.5));
        assert_eq!(un("d:1.0E+25;"), Value::Float(1e25));
        assert!(matches!(un("d:NAN;"), Value::Float(f) if f.is_nan()));
        assert_eq!(un("s:3:\"a\0b\";"), s("a\0b"));
        assert_eq!(un("a:2:{i:0;i:1;s:1:\"5\";b:0;}"), {
            let mut a = Array::new();
            a.push(Value::Int(1));
            a.set(ArrayKey::Int(5), Value::Bool(false));
            Value::Array(a)
        });
        // A reference shared between two elements.
        let Value::Array(a) = un("a:2:{i:0;i:1;i:1;R:2;}") else { panic!() };
        assert!(a.get(&ArrayKey::Int(0)).unwrap().is_ref());
        assert!(a.get(&ArrayKey::Int(1)).unwrap().is_ref());

        let mut it = interp();
        for (text, expected) in [
            ("x", "Error at offset 0 of 1 bytes"),
            ("i:42", "Error at offset 0 of 4 bytes"),
            ("s:5:\"abc\";", "Error at offset 10 of 10 bytes"),
            ("s:3:\"abc\"", "Error at offset 9 of 9 bytes"),
            ("a:1:{i:0;i:1;", "Error at offset 13 of 13 bytes"),
            ("a:1:{d:1.5;i:1;}", "Error at offset 11 of 16 bytes"),
        ] {
            assert_eq!(it.call_function(b"unserialize", &[s(text)]).unwrap(), Value::Bool(false));
            let out = String::from_utf8(it.take_test_output()).unwrap();
            assert!(out.contains(expected), "{text}: {out}");
        }
        // An undeclared class is not an error: php builds
        // `__PHP_Incomplete_Class` and keeps the data.
        let incomplete = it
            .call_function(b"unserialize", &[s("O:3:\"Nop\":1:{s:1:\"a\";i:7;}")])
            .unwrap();
        let Value::Object(o) = &incomplete else { panic!("{incomplete:?}") };
        assert_eq!(o.layout().class_name(), b"__PHP_Incomplete_Class");
        assert_eq!(
            o.get_deref(b"__PHP_Incomplete_Class_Name"),
            Some(Value::string(b"Nop"))
        );
        assert_eq!(o.get_deref(b"a"), Some(Value::Int(7)));
        it.take_test_output();

        assert_eq!(it.call_function(b"unserialize", &[s("i:42;junk")]).unwrap(), Value::Int(42));
        let out = String::from_utf8(it.take_test_output()).unwrap();
        assert!(out.contains("Extra data starting at offset 5 of 9 bytes"), "{out}");
        assert_eq!(it.call_function(b"unserialize", &[s("a:2:{i:0;i:1;}")]).unwrap(), Value::Bool(false));
        let out = String::from_utf8(it.take_test_output()).unwrap();
        assert!(out.contains("Unexpected end of serialized data"), "{out}");
        assert!(out.contains("Error at offset 13 of 14 bytes"), "{out}");
        assert_eq!(it.call_function(b"unserialize", &[s("")]).unwrap(), Value::Bool(false));
        assert_eq!(it.take_test_output(), b"");
    }

    #[test]
    fn unserialize_options_are_validated() {
        let mut opts = Array::new();
        opts.set(ArrayKey::str(b"max_depth"), Value::Int(-1));
        let err = interp().call_function(b"unserialize", &[s("N;"), Value::Array(opts)]).unwrap_err();
        assert_eq!(err.kind(), Some(ErrorKind::ValueError));
        let mut opts = Array::new();
        opts.set(ArrayKey::str(b"max_depth"), Value::Int(2));
        let mut it = interp();
        let r = it
            .call_function(b"unserialize", &[s("a:1:{i:0;a:1:{i:0;a:1:{i:0;i:1;}}}"), Value::Array(opts)])
            .unwrap();
        assert_eq!(r, Value::Bool(false));
        let out = String::from_utf8(it.take_test_output()).unwrap();
        assert!(out.contains("Maximum depth of 2 exceeded"), "{out}");
        assert!(out.contains("Error at offset 23 of 34 bytes"), "{out}");
    }
}
