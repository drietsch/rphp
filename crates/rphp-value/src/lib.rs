//! The runtime value.
//!
//! `Value` is a 16-byte safe-Rust enum: scalars inline (`Null`, `Bool`, `Int`
//! full i64, `Float` f64) and one thin pointer for every heap type — the byte
//! string [`Str`], the copy-on-write [`Array`], the reference-semantics
//! [`Object`], the (interim) [`Closure`], the shared reference cell [`PhpRef`]
//! and the opaque [`Resource`] handle — plus [`Value::Uninit`] for typed
//! properties that were never written. The target `repr(C)` tagged cell over
//! `rphp-gc`/`rphp-heap` (`specs/base/02-value-model.md`) lands later behind
//! this same API. The *operations* here (arithmetic, concatenation,
//! comparison, casts, numeric-string parsing) are the single source of truth
//! that the interpreter and, later, both JIT tiers and const-folding must
//! agree with.
//!
//! **References.** A `Value::Ref` is a handle onto a [`PhpRef`] cell that never
//! itself contains a `Ref`. Every kernel dereferences its operands first, so
//! `$a = 1; $b = &$a; $a === $b` is true and `$a + $b` is `2`. Slots are bound
//! with [`Value::make_ref`] and written with [`Value::assign`] (which writes
//! *through* a bound slot). Containers store `Ref`s as elements; cloning a
//! container shares them (PHP semantics for `$b = $a` when `$a[0]` is a
//! reference).
#![forbid(unsafe_code)]

mod array;
mod closure;
mod object;
mod refs;
mod resource;
mod string;

pub use array::{array_key, Array, ArrayKey};
pub use closure::Closure;
pub use object::{display_class_name, mangled_key};
pub use object::{
    has_pending_destructors, take_pending_destructors, DynProps, Layout, LazyKind, LazyState,
    ObjFlags, Object, ObjectData, ObjectIdAllocator, Payload, PropEntry, PropMeta, Vis,
    WeakObject,
};
pub use refs::PhpRef;
pub use resource::{Resource, ResourceCell, CLOSED_KIND};
pub use string::Str;

use std::borrow::Cow;
use std::fmt;

/// A PHP value. See the crate docs for the representation and the reference
/// rules.
#[derive(Clone, Debug)]
pub enum Value {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(Str),
    Array(Array),
    /// Interim closure value; closures become objects of class `Closure` in
    /// plan E6.
    Closure(Closure),
    Object(Object),
    /// A PHP reference (`&$x`): a handle onto a shared [`PhpRef`] cell. The cell
    /// never contains a `Ref`.
    Ref(PhpRef),
    /// An opaque native handle (`stream`, …).
    Resource(Resource),
    /// A typed property that has never been initialized. It never escapes to
    /// user code: the runtime raises `Error: Typed property X::$p must not be
    /// accessed before initialization` on read, `json_encode`/`foreach`/`(array)`
    /// skip such slots, and the kernels here treat it as `null` only as a
    /// last-resort default.
    Uninit,
}

// Every heap variant is a thin `Rc`, so the enum is a tag plus one word. The
// interpreter's register file and the array entry layout depend on this.
const _: () = assert!(std::mem::size_of::<Value>() == 16);

/// Recoverable value-level errors that surface as PHP `Error`s at runtime.
#[derive(Clone, PartialEq, Debug)]
pub enum ValueError {
    DivisionByZero,
    ModuloByZero,
    /// Unsupported operand types.
    TypeError(&'static str),
}

pub type VResult = Result<Value, ValueError>;

thread_local! {
    /// The object pairs a comparison is inside of right now — php's
    /// recursion protection for `==` over a structure that contains itself.
    static COMPARING: std::cell::RefCell<Vec<(u32, u32)>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// php's `zend_std_compare_objects`: the same object is equal to itself;
/// objects of different classes are *uncomparable* (`1`); objects of one
/// class compare their properties in slot order, the first difference
/// deciding, and a slot only one side has initialized makes them
/// uncomparable too.
///
/// **Known divergence.** php raises `Nesting level too deep - recursive
/// dependency?` when the comparison meets a pair it is already comparing;
/// that pair is treated as equal here, since a value comparison cannot
/// raise.
fn object_spaceship(a: &Object, b: &Object) -> i64 {
    if a.ptr_eq(b) {
        return 0;
    }
    if a.class_id() != b.class_id() {
        return 1;
    }
    let pair = (a.id(), b.id());
    let recursive = COMPARING.with(|c| c.borrow().contains(&pair));
    if recursive {
        return 0;
    }
    COMPARING.with(|c| c.borrow_mut().push(pair));
    let left = a.props_snapshot_with_decl();
    let right = b.props_snapshot_with_decl();
    // Declared slots first, in order — the same class gives both objects the
    // same slots — then the dynamic properties as php compares two symbol
    // tables: the shorter is less, then key by key.
    let mut result = 0;
    for (name, lv, _, ldecl) in left.iter().filter(|e| e.3.is_some()) {
        let Some((_, rv, _, _)) = right
            .iter()
            .find(|(n, _, _, rdecl)| n == name && rdecl == ldecl)
        else {
            result = 1;
            break;
        };
        let r = lv.spaceship(rv);
        if r != 0 {
            result = r;
            break;
        }
    }
    if result == 0 {
        let ldyn: Vec<_> = left.iter().filter(|e| e.3.is_none()).collect();
        let rdyn: Vec<_> = right.iter().filter(|e| e.3.is_none()).collect();
        if ldyn.len() != rdyn.len() {
            result = if ldyn.len() < rdyn.len() { -1 } else { 1 };
        } else {
            for (name, lv, _, _) in ldyn {
                let Some((_, rv, _, _)) = rdyn.iter().find(|(n, _, _, _)| n == name) else {
                    result = 1;
                    break;
                };
                let r = lv.spaceship(rv);
                if r != 0 {
                    result = r;
                    break;
                }
            }
        }
    }
    COMPARING.with(|c| {
        c.borrow_mut().pop();
    });
    result
}

impl Value {
    /// Construct a string value by copying `bytes`.
    pub fn string(bytes: &[u8]) -> Value {
        Value::Str(Str::new(bytes))
    }

    /// An empty array value.
    pub fn empty_array() -> Value {
        Value::Array(Array::new())
    }

    // ----- references -----

    /// Whether this slot is bound to a reference cell.
    pub fn is_ref(&self) -> bool {
        matches!(self, Value::Ref(_))
    }

    /// Whether this is the never-initialized typed-property marker.
    pub fn is_uninit(&self) -> bool {
        matches!(self, Value::Uninit)
    }

    /// The value behind a reference (borrowed for a plain value, a clone of the
    /// cell contents for a `Ref` — an `Rc` bump for heap values).
    pub fn deref(&self) -> Cow<'_, Value> {
        match self {
            Value::Ref(r) => Cow::Owned(r.get()),
            v => Cow::Borrowed(v),
        }
    }

    /// Consume, dereferencing a `Ref` (the value copied out of the cell).
    pub fn unref(self) -> Value {
        match self {
            Value::Ref(r) => r.get(),
            v => v,
        }
    }

    /// Bind `slot` to a reference cell (converting it in place if it is not one
    /// already) and return the cell, for `$b = &$a`, `&$a[0]`, `&$o->p`,
    /// by-reference parameters and `use (&$x)`.
    pub fn make_ref(slot: &mut Value) -> PhpRef {
        if let Value::Ref(r) = slot {
            return r.clone();
        }
        let inner = std::mem::replace(slot, Value::Null);
        let r = PhpRef::new(inner);
        *slot = Value::Ref(r.clone());
        r
    }

    /// By-value assignment into `slot`: writes *through* a bound reference,
    /// otherwise replaces the slot. `v` is dereferenced (assigning from a
    /// reference copies its value; binding is [`Value::make_ref`]).
    pub fn assign(slot: &mut Value, v: Value) {
        let v = v.unref();
        match slot {
            Value::Ref(r) => r.set(v),
            _ => *slot = v,
        }
    }

    // ----- type inspection -----

    /// The `get_debug_type`-style short type name (`int`, `string`, `array`,
    /// `object`, `resource`, `null`).
    pub fn type_name(&self) -> &'static str {
        match self {
            Value::Null | Value::Uninit => "null",
            Value::Bool(_) => "bool",
            Value::Int(_) => "int",
            Value::Float(_) => "float",
            Value::Str(_) => "string",
            Value::Array(_) => "array",
            Value::Closure(_) | Value::Object(_) => "object",
            Value::Resource(_) => "resource",
            Value::Ref(r) => r.borrow().type_name(),
        }
    }

    // ----- casts (8.x semantics for scalars) -----

    pub fn to_bool(&self) -> bool {
        match self {
            Value::Null | Value::Uninit => false,
            Value::Bool(b) => *b,
            Value::Int(i) => *i != 0,
            Value::Float(f) => *f != 0.0,
            // PHP: only "" and "0" are falsy; "0.0", "00", " " are all truthy.
            Value::Str(s) => !(s.is_empty() || s.as_bytes() == b"0"),
            // An array is truthy iff it is non-empty.
            Value::Array(a) => !a.is_empty(),
            // Any object (a closure or class instance) and any resource (even a
            // closed one) is truthy.
            Value::Closure(_) | Value::Object(_) | Value::Resource(_) => true,
            Value::Ref(r) => r.borrow().to_bool(),
        }
    }

    pub fn to_int(&self) -> i64 {
        match self {
            Value::Null | Value::Uninit => 0,
            Value::Bool(b) => *b as i64,
            Value::Int(i) => *i,
            // PHP truncates toward zero; NaN/Inf -> 0.
            Value::Float(f) => {
                if f.is_finite() {
                    f.trunc() as i64
                } else {
                    0
                }
            }
            // `(int)` cast is lenient: it takes the leading numeric run.
            Value::Str(s) => match leading_number(s.as_bytes()) {
                Some(Value::Int(i)) => i,
                Some(Value::Float(f)) => Value::Float(f).to_int(),
                _ => 0,
            },
            // PHP: `(int)` of an array is 0 if empty, else 1.
            Value::Array(a) => i64::from(!a.is_empty()),
            // PHP casts any object to int as 1 (with a notice we do not emit).
            Value::Closure(_) | Value::Object(_) => 1,
            // A resource casts to its id.
            Value::Resource(r) => i64::from(r.id()),
            Value::Ref(r) => r.borrow().to_int(),
        }
    }

    pub fn to_float(&self) -> f64 {
        match self {
            Value::Null | Value::Uninit => 0.0,
            Value::Bool(b) => *b as i64 as f64,
            Value::Int(i) => *i as f64,
            Value::Float(f) => *f,
            Value::Str(s) => match leading_number(s.as_bytes()) {
                Some(v) => v.to_float(),
                None => 0.0,
            },
            Value::Array(a) => f64::from(!a.is_empty()),
            Value::Closure(_) | Value::Object(_) => 1.0,
            Value::Resource(r) => f64::from(r.id()),
            Value::Ref(r) => r.borrow().to_float(),
        }
    }

    /// PHP `echo`/string-cast form, returned as a (possibly lossy) Rust
    /// `String`. For byte-exact output use [`Value::append_php_bytes`]. Float
    /// formatting approximates PHP's default `precision=14` (a documented
    /// divergence axis, ADR-008).
    pub fn to_php_string(&self) -> String {
        match self {
            Value::Str(s) => s.to_string_lossy().into_owned(),
            _ => String::from_utf8_lossy(&self.to_php_bytes()).into_owned(),
        }
    }

    /// Append the byte-exact PHP string form of this value to `out`. This is the
    /// binary-safe path the runtime's `echo` and concatenation use.
    pub fn append_php_bytes(&self, out: &mut Vec<u8>) {
        match self {
            Value::Null | Value::Uninit => {}
            Value::Bool(true) => out.push(b'1'),
            Value::Bool(false) => {}
            Value::Int(i) => out.extend_from_slice(i.to_string().as_bytes()),
            Value::Float(f) => out.extend_from_slice(format_php_float(*f).as_bytes()),
            Value::Str(s) => out.extend_from_slice(s.as_bytes()),
            // PHP stringifies an array to the literal "Array" (with an
            // E_WARNING we do not yet emit).
            Value::Array(_) => out.extend_from_slice(b"Array"),
            // PHP throws when an object without __toString is converted to a
            // string; until the engine has that error channel, an object (a
            // closure or class instance) stringifies to nothing.
            Value::Closure(_) | Value::Object(_) => {}
            Value::Resource(r) => {
                out.extend_from_slice(b"Resource id #");
                out.extend_from_slice(r.id().to_string().as_bytes());
            }
            Value::Ref(r) => r.borrow().append_php_bytes(out),
        }
    }

    /// The byte-exact PHP string form of this value.
    pub fn to_php_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        self.append_php_bytes(&mut out);
        out
    }

    /// PHP `is_numeric`: an int/float, or a fully numeric string.
    pub fn is_numeric(&self) -> bool {
        match self {
            Value::Int(_) | Value::Float(_) => true,
            Value::Str(s) => numeric_string(s.as_bytes()).is_some(),
            Value::Ref(r) => r.borrow().is_numeric(),
            _ => false,
        }
    }

    /// Lenient numeric coercion to an `Int` or `Float` value (never errors;
    /// non-numeric strings and arrays coerce as in a cast). Used by math
    /// builtins; arithmetic uses the stricter [`Value::as_number`].
    pub fn to_number(&self) -> Value {
        match self {
            Value::Int(_) | Value::Float(_) => self.clone(),
            Value::Bool(b) => Value::Int(*b as i64),
            Value::Null | Value::Uninit => Value::Int(0),
            Value::Str(s) => leading_number(s.as_bytes()).unwrap_or(Value::Int(0)),
            Value::Array(a) => Value::Int(i64::from(!a.is_empty())),
            Value::Closure(_) | Value::Object(_) => Value::Int(1),
            Value::Resource(r) => Value::Int(i64::from(r.id())),
            Value::Ref(r) => r.borrow().to_number(),
        }
    }

    /// String concatenation (`.`): byte-exact, never fails for scalars/strings.
    pub fn concat(&self, rhs: &Value) -> Value {
        let mut bytes = self.to_php_bytes();
        rhs.append_php_bytes(&mut bytes);
        Value::Str(Str::from_vec(bytes))
    }

    // ----- arithmetic (numeric scalars; int overflow promotes to float) -----

    fn as_number(&self) -> Result<Value, ValueError> {
        match self {
            Value::Int(_) | Value::Float(_) => Ok(self.clone()),
            Value::Bool(b) => Ok(Value::Int(*b as i64)),
            Value::Null | Value::Uninit => Ok(Value::Int(0)),
            // PHP 8 arithmetic: a leading-numeric string yields its leading
            // number (with an E_WARNING we do not yet emit); a string with no
            // leading number is a TypeError.
            Value::Str(s) => leading_number(s.as_bytes())
                .ok_or(ValueError::TypeError("non-numeric string in arithmetic")),
            Value::Array(_) => Err(ValueError::TypeError("array operand in arithmetic")),
            Value::Closure(_) => Err(ValueError::TypeError("closure operand in arithmetic")),
            Value::Object(_) => Err(ValueError::TypeError("object operand in arithmetic")),
            Value::Resource(_) => Err(ValueError::TypeError("resource operand in arithmetic")),
            Value::Ref(r) => r.borrow().as_number(),
        }
    }

    pub fn add(&self, rhs: &Value) -> VResult {
        let (a, b) = (self.deref(), rhs.deref());
        let (a, b) = (&*a, &*b);
        // PHP `+` on arrays is the union operator, not arithmetic.
        match (a, b) {
            (Value::Array(x), Value::Array(y)) => return Ok(Value::Array(x.union(y))),
            (Value::Array(_), _) | (_, Value::Array(_)) => {
                return Err(ValueError::TypeError("array + non-array"))
            }
            _ => {}
        }
        numeric_binop(a, b, |a, b| a.checked_add(b).map(Value::Int).unwrap_or(Value::Float(a as f64 + b as f64)), |a, b| Value::Float(a + b))
    }

    pub fn sub(&self, rhs: &Value) -> VResult {
        numeric_binop(self, rhs, |a, b| a.checked_sub(b).map(Value::Int).unwrap_or(Value::Float(a as f64 - b as f64)), |a, b| Value::Float(a - b))
    }

    pub fn mul(&self, rhs: &Value) -> VResult {
        numeric_binop(self, rhs, |a, b| a.checked_mul(b).map(Value::Int).unwrap_or(Value::Float(a as f64 * b as f64)), |a, b| Value::Float(a * b))
    }

    pub fn div(&self, rhs: &Value) -> VResult {
        let (a, b) = (self.as_number()?, rhs.as_number()?);
        // PHP: division always considers float unless both ints divide evenly.
        if let (Value::Int(x), Value::Int(y)) = (&a, &b) {
            let (x, y) = (*x, *y);
            if y == 0 {
                return Err(ValueError::DivisionByZero);
            }
            if x % y == 0 {
                return Ok(Value::Int(x / y));
            }
            return Ok(Value::Float(x as f64 / y as f64));
        }
        let (x, y) = (a.to_float(), b.to_float());
        if y == 0.0 {
            return Err(ValueError::DivisionByZero);
        }
        Ok(Value::Float(x / y))
    }

    pub fn rem(&self, rhs: &Value) -> VResult {
        // PHP `%` operates on ints.
        let (a, b) = (self.to_int(), rhs.to_int());
        if b == 0 {
            return Err(ValueError::ModuloByZero);
        }
        Ok(Value::Int(a.wrapping_rem(b)))
    }

    pub fn pow(&self, rhs: &Value) -> VResult {
        let (a, b) = (self.as_number()?, rhs.as_number()?);
        if let (Value::Int(x), Value::Int(y)) = (&a, &b) {
            let (x, y) = (*x, *y);
            if (0..=u32::MAX as i64).contains(&y) {
                if let Some(r) = x.checked_pow(y as u32) {
                    return Ok(Value::Int(r));
                }
            }
        }
        Ok(Value::Float(a.to_float().powf(b.to_float())))
    }

    pub fn neg(&self) -> VResult {
        match self.as_number()? {
            Value::Int(i) => Ok(i.checked_neg().map(Value::Int).unwrap_or(Value::Float(-(i as f64)))),
            Value::Float(f) => Ok(Value::Float(-f)),
            _ => unreachable!(),
        }
    }

    pub fn not(&self) -> Value {
        Value::Bool(!self.to_bool())
    }

    // ----- comparison (PHP 8 scalar semantics) -----

    /// Loose `==`.
    ///
    /// PHP 8 string rules: two numeric strings compare numerically; a number vs
    /// a numeric string compares numerically; a number vs a **non**-numeric
    /// string compares as strings (so `0 == "foo"` is **false**, the 8.0
    /// change). `bool`/`null` operands always compare as booleans. A resource
    /// compares as its id against numbers and strings (`STDIN == 1`), by id
    /// against another resource.
    pub fn loose_eq(&self, rhs: &Value) -> bool {
        use Value::*;
        let (a, b) = (self.deref(), rhs.deref());
        let (lhs, rhs) = (&*a, &*b);
        match (lhs, rhs) {
            (Null | Uninit, Null | Uninit) => true,
            (Bool(_), _) | (_, Bool(_)) => lhs.to_bool() == rhs.to_bool(),
            (Null | Uninit, _) | (_, Null | Uninit) => lhs.to_bool() == rhs.to_bool(),
            (Array(a), Array(b)) => a.loose_eq(b),
            // An array is never loosely equal to a non-array (bool/null already
            // handled above).
            (Array(_), _) | (_, Array(_)) => false,
            // Closures compare by identity; never equal to a non-closure.
            (Closure(a), Closure(b)) => a == b,
            (Closure(_), _) | (_, Closure(_)) => false,
            (Object(a), Object(b)) => object_spaceship(a, b) == 0,
            (Object(_), _) | (_, Object(_)) => false,
            (Resource(a), Resource(b)) => a.id() == b.id(),
            (Resource(r), _) => Int(i64::from(r.id())).loose_eq(&resource_operand(rhs)),
            (_, Resource(r)) => resource_operand(lhs).loose_eq(&Int(i64::from(r.id()))),
            (Str(a), Str(b)) => {
                match (numeric_string(a.as_bytes()), numeric_string(b.as_bytes())) {
                    (Some(x), Some(y)) => x.loose_eq(&y),
                    _ => a == b,
                }
            }
            (Str(a), _) => match numeric_string(a.as_bytes()) {
                Some(x) => x.loose_eq(rhs),
                None => a.as_bytes() == rhs.to_php_bytes().as_slice(),
            },
            (_, Str(b)) => match numeric_string(b.as_bytes()) {
                Some(y) => lhs.loose_eq(&y),
                None => lhs.to_php_bytes().as_slice() == b.as_bytes(),
            },
            (Int(a), Int(b)) => a == b,
            // any float involved -> compare as floats
            _ => lhs.to_float() == rhs.to_float(),
        }
    }

    /// Strict `===` (same type and value). References compare their contents;
    /// objects and resources by identity.
    pub fn identical(&self, rhs: &Value) -> bool {
        use Value::*;
        let (a, b) = (self.deref(), rhs.deref());
        match (&*a, &*b) {
            (Null | Uninit, Null | Uninit) => true,
            (Bool(a), Bool(b)) => a == b,
            (Int(a), Int(b)) => a == b,
            (Float(a), Float(b)) => a == b,
            (Str(a), Str(b)) => a == b,
            (Array(a), Array(b)) => a.identical(b),
            (Closure(a), Closure(b)) => a == b,
            (Object(a), Object(b)) => a == b,
            (Resource(a), Resource(b)) => a.ptr_eq(b),
            _ => false,
        }
    }

    /// `<=>` spaceship: -1, 0, 1. `bool`/`null` operands compare as booleans;
    /// strings follow the same numeric-vs-lexical rules as `loose_eq`; a
    /// resource compares as its id.
    pub fn spaceship(&self, rhs: &Value) -> i64 {
        use std::cmp::Ordering;
        use Value::*;
        let (a, b) = (self.deref(), rhs.deref());
        let (lhs, rhs) = (&*a, &*b);
        match (lhs, rhs) {
            (Bool(_), _) | (_, Bool(_)) | (Null | Uninit, _) | (_, Null | Uninit) => {
                bool_cmp(lhs.to_bool(), rhs.to_bool())
            }
            (Array(a), Array(b)) => a.spaceship(b),
            // An array is greater than any non-array (bool/null handled above).
            (Array(_), _) => 1,
            (_, Array(_)) => -1,
            // Closures are uncomparable beyond identity; treat like objects
            // (greater than non-objects), equal only to themselves.
            (Closure(a), Closure(b)) => i64::from(a != b),
            (Closure(_), _) => 1,
            (_, Closure(_)) => -1,
            // Objects of one class compare property by property; of two
            // classes they are uncomparable, which php spells `1`.
            (Object(a), Object(b)) => object_spaceship(a, b),
            (Object(_), _) => 1,
            (_, Object(_)) => -1,
            (Resource(a), Resource(b)) => int_cmp(i64::from(a.id()), i64::from(b.id())),
            (Resource(r), _) => Int(i64::from(r.id())).spaceship(&resource_operand(rhs)),
            (_, Resource(r)) => resource_operand(lhs).spaceship(&Int(i64::from(r.id()))),
            (Str(a), Str(b)) => {
                match (numeric_string(a.as_bytes()), numeric_string(b.as_bytes())) {
                    (Some(x), Some(y)) => x.spaceship(&y),
                    _ => byte_cmp(a.as_bytes(), b.as_bytes()),
                }
            }
            (Str(a), _) => match numeric_string(a.as_bytes()) {
                Some(x) => x.spaceship(rhs),
                None => byte_cmp(a.as_bytes(), &rhs.to_php_bytes()),
            },
            (_, Str(b)) => match numeric_string(b.as_bytes()) {
                Some(y) => lhs.spaceship(&y),
                None => byte_cmp(&lhs.to_php_bytes(), b.as_bytes()),
            },
            _ => {
                // Both numeric (Int/Float).
                if lhs.identical(rhs) {
                    return 0;
                }
                let (a, b) = (lhs.to_float(), rhs.to_float());
                match a.partial_cmp(&b) {
                    Some(Ordering::Less) => -1,
                    Some(Ordering::Greater) => 1,
                    _ => 0,
                }
            }
        }
    }

    pub fn lt(&self, rhs: &Value) -> bool { self.spaceship(rhs) < 0 }
    pub fn le(&self, rhs: &Value) -> bool { self.spaceship(rhs) <= 0 }
    pub fn gt(&self, rhs: &Value) -> bool { self.spaceship(rhs) > 0 }
    pub fn ge(&self, rhs: &Value) -> bool { self.spaceship(rhs) >= 0 }
}

impl PartialEq for Value {
    /// Rust-level structural equality for tests and tables — **not** PHP's `==`
    /// (that is [`Value::loose_eq`]). References compare their contents;
    /// arrays order-sensitively; objects, closures and resources by identity.
    fn eq(&self, other: &Value) -> bool {
        use Value::*;
        match (&*self.deref(), &*other.deref()) {
            (Null, Null) | (Uninit, Uninit) => true,
            (Bool(a), Bool(b)) => a == b,
            (Int(a), Int(b)) => a == b,
            (Float(a), Float(b)) => a == b,
            (Str(a), Str(b)) => a == b,
            (Array(a), Array(b)) => a == b,
            (Closure(a), Closure(b)) => a == b,
            (Object(a), Object(b)) => a == b,
            (Resource(a), Resource(b)) => a == b,
            _ => false,
        }
    }
}

/// The scalar a non-resource operand becomes when compared against a resource
/// (`_zendi_convert_scalar_to_number_silent`): strings yield their leading
/// number or `0`, everything else its numeric cast.
fn resource_operand(v: &Value) -> Value {
    match v {
        Value::Str(s) => leading_number(s.as_bytes()).unwrap_or(Value::Int(0)),
        other => other.to_number(),
    }
}

fn numeric_binop(
    lhs: &Value,
    rhs: &Value,
    on_int: impl Fn(i64, i64) -> Value,
    on_float: impl Fn(f64, f64) -> Value,
) -> VResult {
    let (a, b) = (lhs.as_number()?, rhs.as_number()?);
    match (a, b) {
        (Value::Int(x), Value::Int(y)) => Ok(on_int(x, y)),
        (a, b) => Ok(on_float(a.to_float(), b.to_float())),
    }
}

/// `false < true`, returning -1/0/1.
fn bool_cmp(a: bool, b: bool) -> i64 {
    (a as i64) - (b as i64)
}

/// Integer order, returning -1/0/1.
fn int_cmp(a: i64, b: i64) -> i64 {
    match a.cmp(&b) {
        std::cmp::Ordering::Less => -1,
        std::cmp::Ordering::Equal => 0,
        std::cmp::Ordering::Greater => 1,
    }
}

/// Lexical byte comparison, returning -1/0/1.
fn byte_cmp(a: &[u8], b: &[u8]) -> i64 {
    use std::cmp::Ordering;
    match a.cmp(b) {
        Ordering::Less => -1,
        Ordering::Equal => 0,
        Ordering::Greater => 1,
    }
}

/// Whitespace PHP's numeric-string parsing tolerates (leading and, since 8.0,
/// trailing).
fn is_php_space(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c)
}

/// Parse a PHP number out of `bytes`. With `require_full`, the *entire* string
/// (modulo leading/trailing whitespace) must be the number — this is the
/// `is_numeric_string` predicate used by comparisons. Without it, only a leading
/// numeric run is consumed — the lenient form used by `(int)`/`(float)` casts
/// and PHP-8 arithmetic coercion. Returns `Int` when there is no fractional or
/// exponent part (overflowing `i64` promotes to `Float`, as PHP does), else
/// `Float`. Hex/binary/octal *strings* are not numeric in PHP 7+, matching here.
fn parse_number(bytes: &[u8], require_full: bool) -> Option<Value> {
    let n = bytes.len();
    let mut i = 0;
    while i < n && is_php_space(bytes[i]) {
        i += 1;
    }
    let num_start = i;
    if i < n && (bytes[i] == b'+' || bytes[i] == b'-') {
        i += 1;
    }
    let mut has_digits = false;
    let mut is_float = false;
    while i < n && bytes[i].is_ascii_digit() {
        i += 1;
        has_digits = true;
    }
    if i < n && bytes[i] == b'.' {
        is_float = true;
        i += 1;
        while i < n && bytes[i].is_ascii_digit() {
            i += 1;
            has_digits = true;
        }
    }
    if !has_digits {
        return None;
    }
    if i < n && (bytes[i] == b'e' || bytes[i] == b'E') {
        // An exponent only counts if it has at least one digit; otherwise the
        // `e` is trailing garbage (e.g. `(int)"1e"` == 1).
        let mut j = i + 1;
        if j < n && (bytes[j] == b'+' || bytes[j] == b'-') {
            j += 1;
        }
        let mut exp_digits = false;
        while j < n && bytes[j].is_ascii_digit() {
            j += 1;
            exp_digits = true;
        }
        if exp_digits {
            is_float = true;
            i = j;
        }
    }
    let num_end = i;
    if require_full {
        let mut k = num_end;
        while k < n && is_php_space(bytes[k]) {
            k += 1;
        }
        if k != n {
            return None;
        }
    }
    // The numeric run is pure ASCII, so this never fails.
    let s = std::str::from_utf8(&bytes[num_start..num_end]).ok()?;
    if is_float {
        s.parse::<f64>().ok().map(Value::Float)
    } else {
        match s.parse::<i64>() {
            Ok(v) => Some(Value::Int(v)),
            Err(_) => s.parse::<f64>().ok().map(Value::Float),
        }
    }
}

/// `is_numeric_string`: the whole string is a number (PHP 8 leading/trailing
/// whitespace allowed).
fn numeric_string(bytes: &[u8]) -> Option<Value> {
    parse_number(bytes, true)
}

/// The leading numeric run, for casts and arithmetic coercion.
fn leading_number(bytes: &[u8]) -> Option<Value> {
    parse_number(bytes, false)
}

/// PHP's default float-to-string: a `%.14G`-style format matching the default
/// `precision=14` ini setting that `echo`/string-cast use. Fixed notation for
/// exponents in `-4 ..= 13`, otherwise scientific with an uppercase `E`, a
/// signed exponent, and a forced `.0` mantissa. (`serialize_precision=-1`
/// shortest-round-trip, used by `var_export`/`json_encode`, is a separate path
/// added later.) Verified against stock PHP 8.5 by the differential suite.
fn format_php_float(f: f64) -> String {
    if f.is_nan() {
        return "NAN".to_string();
    }
    if f.is_infinite() {
        return if f < 0.0 { "-INF".to_string() } else { "INF".to_string() };
    }
    if f == 0.0 {
        return if f.is_sign_negative() { "-0" } else { "0" }.to_string();
    }

    const PREC: i32 = 14;
    let neg = f < 0.0;

    // Canonical 14-significant-digit scientific form: "d.ddddddddddddde{E}".
    // Rust's LowerExp yields one leading digit, lowercase `e`, no `+`, and no
    // leading zeros in the exponent — exactly what we need to re-lay-out.
    let sci = format!("{:.*e}", (PREC - 1) as usize, f.abs());
    let (mant, exp) = sci.split_once('e').expect("LowerExp always has 'e'");
    let e: i32 = exp.parse().expect("valid exponent");
    let digits: String = mant.chars().filter(|&c| c != '.').collect(); // PREC digits

    let mut out = String::new();
    if neg {
        out.push('-');
    }

    if !(-4..PREC).contains(&e) {
        // Scientific.
        out.push_str(&digits[0..1]);
        out.push('.');
        let frac = digits[1..].trim_end_matches('0');
        out.push_str(if frac.is_empty() { "0" } else { frac });
        out.push('E');
        out.push(if e >= 0 { '+' } else { '-' });
        out.push_str(&e.unsigned_abs().to_string());
    } else if e >= 0 {
        // Fixed, magnitude >= 1.
        let intlen = (e + 1) as usize;
        if intlen >= digits.len() {
            out.push_str(&digits);
            out.push_str(&"0".repeat(intlen - digits.len()));
        } else {
            out.push_str(&digits[..intlen]);
            let frac = digits[intlen..].trim_end_matches('0');
            if !frac.is_empty() {
                out.push('.');
                out.push_str(frac);
            }
        }
    } else {
        // Fixed, magnitude < 1: "0.00…digits".
        let lead_zeros = (-e - 1) as usize;
        let frac_full = format!("{}{}", "0".repeat(lead_zeros), digits);
        let frac = frac_full.trim_end_matches('0');
        out.push_str("0.");
        out.push_str(if frac.is_empty() { "0" } else { frac });
    }
    out
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_php_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn value_is_sixteen_bytes() {
        assert_eq!(std::mem::size_of::<Value>(), 16);
        assert_eq!(std::mem::size_of::<Option<Value>>(), 16);
    }

    #[test]
    fn uninit_behaves_as_null_in_kernels() {
        assert_eq!(Value::Uninit.type_name(), "null");
        assert!(!Value::Uninit.to_bool());
        assert_eq!(Value::Uninit.to_int(), 0);
        assert_eq!(Value::Uninit.to_php_bytes(), b"");
        assert!(Value::Uninit.loose_eq(&Value::Null));
        assert!(Value::Uninit.identical(&Value::Null));
        assert!(Value::Uninit.is_uninit());
        assert_ne!(Value::Uninit, Value::Null); // distinct at the Rust level
    }

    #[test]
    fn int_overflow_promotes_to_float() {
        let r = Value::Int(i64::MAX).add(&Value::Int(1)).unwrap();
        assert!(matches!(r, Value::Float(_)));
    }

    #[test]
    fn div_even_is_int_else_float() {
        assert_eq!(Value::Int(6).div(&Value::Int(3)).unwrap(), Value::Int(2));
        assert_eq!(Value::Int(7).div(&Value::Int(2)).unwrap(), Value::Float(3.5));
        assert_eq!(Value::Int(1).div(&Value::Int(0)), Err(ValueError::DivisionByZero));
    }

    #[test]
    fn php8_bool_vs_int_compare() {
        // 0 == false  (true) ; 1 == true (true)
        assert!(Value::Int(0).loose_eq(&Value::Bool(false)));
        assert!(Value::Int(1).loose_eq(&Value::Bool(true)));
        assert!(!Value::Int(2).identical(&Value::Float(2.0)));
    }

    #[test]
    fn spaceship_and_string() {
        assert_eq!(Value::Int(1).spaceship(&Value::Int(2)), -1);
        assert_eq!(Value::Float(2.0).to_php_string(), "2");
        assert_eq!(Value::Float(3.5).to_php_string(), "3.5");
        assert_eq!(Value::Bool(true).to_php_string(), "1");
        assert_eq!(Value::Bool(false).to_php_string(), "");
        assert_eq!(Value::Null.to_php_string(), "");
    }

    #[test]
    fn float_formatting_matches_php() {
        let f = |x: f64| Value::Float(x).to_php_string();
        assert_eq!(f(5.0), "5");
        assert_eq!(f(3.5), "3.5");
        assert_eq!(f(0.1), "0.1");
        assert_eq!(f(1.0 / 3.0), "0.33333333333333");
        assert_eq!(f(100.0), "100");
        assert_eq!(f(1234567890.5), "1234567890.5");
        assert_eq!(f(1e14), "1.0E+14");
        assert_eq!(f(1e20), "1.0E+20");
        assert_eq!(f(1e-5), "1.0E-5");
        // i64::MAX + 1 promoted to float, as PHP prints it.
        assert_eq!(f(9223372036854775808.0), "9.2233720368548E+18");
        assert_eq!(f(-3.5), "-3.5");
    }

    #[test]
    fn pow_and_mod() {
        assert_eq!(Value::Int(2).pow(&Value::Int(10)).unwrap(), Value::Int(1024));
        assert_eq!(Value::Int(7).rem(&Value::Int(3)).unwrap(), Value::Int(1));
        assert_eq!(Value::Int(1).rem(&Value::Int(0)), Err(ValueError::ModuloByZero));
    }

    fn s(bytes: &str) -> Value {
        Value::string(bytes.as_bytes())
    }

    #[test]
    fn concat_stringifies_operands() {
        assert_eq!(s("foo").concat(&s("bar")), s("foobar"));
        assert_eq!(s("x=").concat(&Value::Int(5)), s("x=5"));
        assert_eq!(Value::Int(1).concat(&Value::Int(2)), s("12"));
        assert_eq!(s("v=").concat(&Value::Float(3.5)), s("v=3.5"));
        assert_eq!(s("a").concat(&Value::Bool(true)).concat(&Value::Null), s("a1"));
    }

    #[test]
    fn string_truthiness() {
        assert!(!s("").to_bool());
        assert!(!s("0").to_bool());
        // "0.0", "00", and " " are all truthy in PHP.
        assert!(s("0.0").to_bool());
        assert!(s("00").to_bool());
        assert!(s(" ").to_bool());
        assert!(s("false").to_bool());
    }

    #[test]
    fn string_numeric_casts() {
        assert_eq!(s("123abc").to_int(), 123);
        assert_eq!(s("abc").to_int(), 0);
        assert_eq!(s("12.9").to_int(), 12);
        assert_eq!(s("  -7 ").to_int(), -7);
        assert_eq!(s("1.5e3xyz").to_float(), 1500.0);
        assert_eq!(s("0x1A").to_int(), 0); // hex strings are not numeric
    }

    #[test]
    fn string_arithmetic_php8() {
        // Fully numeric strings add as numbers.
        assert_eq!(s("10").add(&Value::Int(5)).unwrap(), Value::Int(15));
        assert_eq!(s("2.5").add(&s("2.5")).unwrap(), Value::Float(5.0));
        // Leading-numeric string yields its leading number (warning deferred).
        assert_eq!(s("10 apples").add(&Value::Int(5)).unwrap(), Value::Int(15));
        // A non-numeric string in arithmetic is a TypeError.
        assert_eq!(
            s("apples").add(&Value::Int(1)),
            Err(ValueError::TypeError("non-numeric string in arithmetic"))
        );
        assert!(s("").add(&Value::Int(1)).is_err());
    }

    #[test]
    fn string_comparison_php8() {
        // Two numeric strings compare numerically.
        assert!(s("10").loose_eq(&s("1e1")));
        assert_eq!(s("10").spaceship(&s("9")), 1);
        // Number vs numeric string -> numeric.
        assert!(Value::Int(5).loose_eq(&s("5")));
        assert!(!Value::Int(5).loose_eq(&s("5abc")));
        // The PHP 8 change: number vs non-numeric string compares as strings.
        assert!(!Value::Int(0).loose_eq(&s("foo")));
        assert!(Value::Int(0).loose_eq(&s("0")));
        // Non-numeric strings compare lexically.
        assert_eq!(s("abc").spaceship(&s("abd")), -1);
        assert!(s("abc").identical(&s("abc")));
        assert!(!Value::Int(2).identical(&s("2")));
        // bool/null operands always compare as booleans.
        assert!(s("anything").loose_eq(&Value::Bool(true)));
        assert!(s("0").loose_eq(&Value::Bool(false)));
    }

    fn arr(items: &[Value]) -> Value {
        let mut a = Array::new();
        for v in items {
            a.push(v.clone());
        }
        Value::Array(a)
    }

    #[test]
    fn array_truthiness_and_casts() {
        assert!(!Value::empty_array().to_bool());
        assert!(arr(&[Value::Int(0)]).to_bool()); // non-empty even with a falsy element
        assert_eq!(Value::empty_array().to_int(), 0);
        assert_eq!(arr(&[Value::Int(7)]).to_int(), 1);
        assert_eq!(arr(&[Value::Int(7)]).to_php_string(), "Array");
    }

    #[test]
    fn array_union_and_arithmetic_error() {
        // [1,2] + [9,9,9] => keys 0,1 from left, key 2 from right => [1,2,9]
        let u = arr(&[Value::Int(1), Value::Int(2)])
            .add(&arr(&[Value::Int(9), Value::Int(9), Value::Int(9)]))
            .unwrap();
        assert_eq!(u, arr(&[Value::Int(1), Value::Int(2), Value::Int(9)]));
        // array + scalar is a TypeError.
        assert!(Value::empty_array().add(&Value::Int(1)).is_err());
        // other arithmetic on arrays errors too.
        assert!(arr(&[Value::Int(1)]).sub(&arr(&[Value::Int(1)])).is_err());
    }

    #[test]
    fn array_comparison() {
        // Order-independent loose ==, order-sensitive ===.
        let mut a = Array::new();
        a.set(ArrayKey::Int(0), Value::Int(1));
        a.set(ArrayKey::Int(1), Value::Int(2));
        let mut b = Array::new();
        b.set(ArrayKey::Int(1), Value::Int(2));
        b.set(ArrayKey::Int(0), Value::Int(1));
        assert!(Value::Array(a.clone()).loose_eq(&Value::Array(b.clone())));
        assert!(!Value::Array(a).identical(&Value::Array(b))); // different order
        // Fewer elements compares less; array > non-array.
        assert_eq!(arr(&[Value::Int(1)]).spaceship(&arr(&[Value::Int(1), Value::Int(2)])), -1);
        assert_eq!(Value::empty_array().spaceship(&Value::Int(5)), 1);
        assert!(!Value::empty_array().loose_eq(&Value::Int(0)));
    }

    #[test]
    fn arrays_with_reference_elements_compare_by_contents() {
        // $d = [1, 2]; $r = &$d[1]; var_dump($d == [1, 2], $d === [1, 2]);  => true, true
        let mut d = Array::new();
        d.push(Value::Int(1));
        d.push(Value::Int(2));
        let _r = d.get_ref(ArrayKey::Int(1));
        let plain = arr(&[Value::Int(1), Value::Int(2)]);
        assert!(Value::Array(d.clone()).loose_eq(&plain));
        assert!(Value::Array(d.clone()).identical(&plain));
        assert_eq!(Value::Array(d), plain);
    }
}
