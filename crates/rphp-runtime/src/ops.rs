//! Value-level operator semantics the dispatch loop delegates to: arithmetic
//! with php's operand diagnostics, bitwise and shift operators, `++`/`--`,
//! casts, array element reads and writes (with warnings and the
//! autovivification rules), string offsets, and `match`/`instanceof`
//! helpers. Everything here returns `Result<_, Unwind>` and emits its
//! warnings through the diagnostics channel.

use rphp_bytecode::{AssignOpKind, CastKind};
use rphp_value::{array_key, Array, ArrayKey, Object, Str, Value, ValueError};

use crate::registry::Unwind;
use crate::Interp;

/// php's `zend_zval_value_name`: the name a value has in messages such as
/// `Call to a member function m() on int` / `… on true`.
pub fn value_name(v: &Value) -> String {
    match &*v.deref() {
        Value::Null | Value::Uninit => "null".to_string(),
        Value::Bool(true) => "true".to_string(),
        Value::Bool(false) => "false".to_string(),
        Value::Int(_) => "int".to_string(),
        Value::Float(_) => "float".to_string(),
        Value::Str(_) => "string".to_string(),
        Value::Array(_) => "array".to_string(),
        Value::Closure(_) => "Closure".to_string(),
        Value::Object(o) => {
            String::from_utf8_lossy(rphp_value::display_class_name(o.layout().class_name()))
                .into_owned()
        }
        Value::Resource(_) => "resource".to_string(),
        Value::Ref(_) => unreachable!("deref'd above"),
    }
}

/// php's `zend_zval_type_name`: the operand type in `Unsupported operand
/// types: array + int` (bools are `bool`, objects their class name).
fn operand_type_name(v: &Value) -> String {
    match &*v.deref() {
        Value::Bool(_) => "bool".to_string(),
        other => value_name(other),
    }
}

/// A leading numeric prefix that is not the whole string (`"12abc"`): php
/// uses the prefix and warns `A non-numeric value encountered`.
fn leading_numeric_but_not_numeric(s: &[u8]) -> bool {
    let n = s.len();
    let mut i = 0;
    while i < n && matches!(s[i], b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c) {
        i += 1;
    }
    if i < n && (s[i] == b'+' || s[i] == b'-') {
        i += 1;
    }
    let mut digits = false;
    while i < n && s[i].is_ascii_digit() {
        i += 1;
        digits = true;
    }
    if i < n && s[i] == b'.' {
        i += 1;
        while i < n && s[i].is_ascii_digit() {
            i += 1;
            digits = true;
        }
    }
    digits
}

/// The operator spelling in `Unsupported operand types` messages.
fn arith_symbol(op: AssignOpKind) -> &'static str {
    match op {
        AssignOpKind::Add => "+",
        AssignOpKind::Sub => "-",
        AssignOpKind::Mul => "*",
        AssignOpKind::Div => "/",
        AssignOpKind::Mod => "%",
        AssignOpKind::Pow => "**",
        AssignOpKind::Concat => ".",
        AssignOpKind::BitAnd => "&",
        AssignOpKind::BitOr => "|",
        AssignOpKind::BitXor => "^",
        AssignOpKind::Shl => "<<",
        AssignOpKind::Shr => ">>",
        AssignOpKind::Coalesce => "??",
    }
}

/// A diagnostic an element store asks its caller to emit.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SetNotice {
    /// Nothing to report.
    None,
    /// `Automatic conversion of false to array is deprecated`.
    FalseToArray,
    /// `Only the first byte will be assigned to the string offset`.
    FirstByteOnly,
}

impl Interp {
    /// The two operands of a loose comparison, after php's one conversion
    /// that a value comparison cannot make on its own: an **object beside a
    /// number** is cast to that number's type — `1` with `Notice: Object of
    /// class C could not be converted to int` (or `float`) — so `$obj == 1`
    /// is true and `$obj <=> 5` is `-1`. Every other pairing (bool, null,
    /// string, array, another object) compares as `Value` already does.
    pub fn cmp_operands(&mut self, l: Value, r: Value) -> Result<(Value, Value), Unwind> {
        let (l, r) = self.lazy_beside_object(l, r)?;
        let l = self.object_beside_number(&l, &r)?;
        let r = self.object_beside_number(&r, &l)?;
        Ok((l, r))
    }

    /// Two distinct objects compare property by property, which initializes
    /// a lazy one and compares a proxy's real instance; the same instance on
    /// both sides is equal without any of that.
    fn lazy_beside_object(&mut self, l: Value, r: Value) -> Result<(Value, Value), Unwind> {
        let (Value::Object(a), Value::Object(b)) = (&*l.deref(), &*r.deref()) else {
            return Ok((l, r));
        };
        if a.ptr_eq(b) || (!a.is_lazy() && !b.is_lazy()) {
            return Ok((l, r));
        }
        let a = self.lazy_resolve(&a.clone())?;
        let b = self.lazy_resolve(&b.clone())?;
        Ok((Value::Object(a), Value::Object(b)))
    }

    fn object_beside_number(&mut self, v: &Value, other: &Value) -> Result<Value, Unwind> {
        let (obj, as_float) = match (&*v.deref(), &*other.deref()) {
            (Value::Object(o), Value::Int(_)) => (o.clone(), false),
            (Value::Object(o), Value::Float(_)) => (o.clone(), true),
            // Beside a string, a `Stringable` object compares as its
            // `__toString()`; any other object stays an object (and is then
            // simply unequal).
            (Value::Object(o), Value::Str(_)) => {
                let o = o.clone();
                if self.class_of(&o).magic.contains(crate::class::MagicFlags::TOSTRING) {
                    return Ok(Value::Str(self.object_to_string(&o)?));
                }
                return Ok(v.deref().into_owned());
            }
            _ => return Ok(v.deref().into_owned()),
        };
        let class = String::from_utf8_lossy(obj.layout().class_name()).into_owned();
        self.notice(&format!(
            "Object of class {class} could not be converted to {}",
            if as_float { "float" } else { "int" }
        ))?;
        Ok(if as_float { Value::Float(1.0) } else { Value::Int(1) })
    }

    /// Warn for a leading-numeric string operand (`"12abc" + 1`).
    fn warn_non_numeric(&mut self, v: &Value) -> Result<(), Unwind> {
        if let Value::Str(s) = &*v.deref() {
            if !v.is_numeric() && leading_numeric_but_not_numeric(s.as_bytes()) {
                self.warn("A non-numeric value encountered")?;
            }
        }
        Ok(())
    }

    /// Map a value-level arithmetic fault to php's throwable.
    fn arith_fault(op: AssignOpKind, a: &Value, b: &Value, err: ValueError) -> Unwind {
        match err {
            ValueError::DivisionByZero => Unwind::division_by_zero("Division by zero"),
            ValueError::ModuloByZero => Unwind::division_by_zero("Modulo by zero"),
            ValueError::TypeError(_) => Unwind::type_error(format!(
                "Unsupported operand types: {} {} {}",
                operand_type_name(a),
                arith_symbol(op),
                operand_type_name(b)
            )),
        }
    }

    /// A binary arithmetic / string / bitwise operator with php's
    /// diagnostics: leading-numeric strings warn, non-numeric ones and
    /// arrays/objects are a `TypeError`.
    pub fn binary_op(&mut self, op: AssignOpKind, a: &Value, b: &Value) -> Result<Value, Unwind> {
        let a = a.deref().into_owned();
        let b = b.deref().into_owned();
        match op {
            AssignOpKind::Concat => {
                if matches!(a, Value::Object(_) | Value::Array(_))
                    || matches!(b, Value::Object(_) | Value::Array(_))
                {
                    let mut out = self.to_string(&a)?.as_bytes().to_vec();
                    out.extend_from_slice(self.to_string(&b)?.as_bytes());
                    return Ok(Value::Str(Str::from_vec(out)));
                }
                Ok(a.concat(&b))
            }
            AssignOpKind::Add
            | AssignOpKind::Sub
            | AssignOpKind::Mul
            | AssignOpKind::Div
            | AssignOpKind::Mod
            | AssignOpKind::Pow => {
                // `[] + []` is the array union; every other array operand is a TypeError.
                if op == AssignOpKind::Add {
                    if let (Value::Array(x), Value::Array(y)) = (&a, &b) {
                        return Ok(Value::Array(x.union(y)));
                    }
                }
                self.warn_non_numeric(&a)?;
                self.warn_non_numeric(&b)?;
                if op == AssignOpKind::Mod {
                    // `%` converts its operands to int; a fractional float
                    // loses precision (php 8.1 deprecation).
                    for v in [&a, &b] {
                        if let Value::Float(f) = v {
                            if f.fract() != 0.0 && f.is_finite() {
                                self.deprecated(&format!(
                                    "Implicit conversion from float {} to int loses precision",
                                    v.to_php_string()
                                ))?;
                            }
                        }
                    }
                }
                let r = match op {
                    AssignOpKind::Add => a.add(&b),
                    AssignOpKind::Sub => a.sub(&b),
                    AssignOpKind::Mul => a.mul(&b),
                    AssignOpKind::Div => a.div(&b),
                    AssignOpKind::Mod => a.rem(&b),
                    _ => a.pow(&b),
                };
                r.map_err(|e| Self::arith_fault(op, &a, &b, e))
            }
            AssignOpKind::BitAnd | AssignOpKind::BitOr | AssignOpKind::BitXor => {
                if let (Value::Str(x), Value::Str(y)) = (&a, &b) {
                    let (x, y) = (x.as_bytes(), y.as_bytes());
                    let bytes: Vec<u8> = match op {
                        AssignOpKind::BitAnd => x.iter().zip(y).map(|(p, q)| p & q).collect(),
                        AssignOpKind::BitOr => {
                            let (long, short) = if x.len() >= y.len() { (x, y) } else { (y, x) };
                            long.iter()
                                .enumerate()
                                .map(|(i, p)| p | short.get(i).copied().unwrap_or(0))
                                .collect()
                        }
                        _ => x.iter().zip(y).map(|(p, q)| p ^ q).collect(),
                    };
                    return Ok(Value::Str(Str::from_vec(bytes)));
                }
                let x = self.int_operand(&a, &b, op)?;
                let y = self.int_operand(&b, &a, op)?;
                Ok(Value::Int(match op {
                    AssignOpKind::BitAnd => x & y,
                    AssignOpKind::BitOr => x | y,
                    _ => x ^ y,
                }))
            }
            AssignOpKind::Shl | AssignOpKind::Shr => {
                let x = self.int_operand(&a, &b, op)?;
                let y = self.int_operand(&b, &a, op)?;
                if y < 0 {
                    return Err(Unwind::arithmetic_error("Bit shift by negative number"));
                }
                Ok(Value::Int(if op == AssignOpKind::Shl {
                    if y >= 64 {
                        0
                    } else {
                        x.wrapping_shl(y as u32)
                    }
                } else if y >= 64 {
                    if x < 0 {
                        -1
                    } else {
                        0
                    }
                } else {
                    x >> y
                }))
            }
            AssignOpKind::Coalesce => Ok(if matches!(a, Value::Null | Value::Uninit) { b } else { a }),
        }
    }

    /// An integer operand of a bitwise/shift operator (`other` is the
    /// partner, for the message).
    fn int_operand(&mut self, v: &Value, other: &Value, op: AssignOpKind) -> Result<i64, Unwind> {
        match v {
            Value::Int(i) => Ok(*i),
            Value::Float(f) => {
                if f.fract() != 0.0 && f.is_finite() {
                    self.deprecated(&format!(
                        "Implicit conversion from float {} to int loses precision",
                        v.to_php_string()
                    ))?;
                }
                Ok(v.to_int())
            }
            Value::Null | Value::Uninit | Value::Bool(_) => Ok(v.to_int()),
            Value::Str(_) => {
                if !v.is_numeric() {
                    if !leading_numeric_but_not_numeric(v.to_php_bytes().as_slice()) {
                        return Err(Unwind::type_error(format!(
                            "Unsupported operand types: {} {} {}",
                            operand_type_name(v),
                            arith_symbol(op),
                            operand_type_name(other)
                        )));
                    }
                    self.warn("A non-numeric value encountered")?;
                }
                Ok(v.to_number().to_int())
            }
            _ => Err(Unwind::type_error(format!(
                "Unsupported operand types: {} {} {}",
                operand_type_name(v),
                arith_symbol(op),
                operand_type_name(other)
            ))),
        }
    }

    /// `~$x`.
    pub fn bit_not(&mut self, v: &Value) -> Result<Value, Unwind> {
        match &*v.deref() {
            Value::Int(i) => Ok(Value::Int(!i)),
            Value::Float(f) => {
                if f.fract() != 0.0 && f.is_finite() {
                    self.deprecated(&format!(
                        "Implicit conversion from float {} to int loses precision",
                        v.to_php_string()
                    ))?;
                }
                Ok(Value::Int(!v.to_int()))
            }
            Value::Str(s) => Ok(Value::Str(Str::from_vec(
                s.as_bytes().iter().map(|b| !b).collect(),
            ))),
            other => Err(Unwind::type_error(format!(
                "Cannot perform bitwise not on {}",
                operand_type_name(other)
            ))),
        }
    }

    /// Unary `+$x` (php: `$x * 1`).
    pub fn unary_plus(&mut self, v: &Value) -> Result<Value, Unwind> {
        let one = Value::Int(1);
        self.binary_op(AssignOpKind::Mul, v, &one)
    }

    /// Unary `-$x` (php: `$x * -1`).
    pub fn unary_neg(&mut self, v: &Value) -> Result<Value, Unwind> {
        let m1 = Value::Int(-1);
        self.binary_op(AssignOpKind::Mul, v, &m1)
    }

    /// `Array to string conversion` warning for `echo`/concat of an array.
    pub fn check_array_to_string(&mut self, v: &Value) -> Result<(), Unwind> {
        if matches!(&*v.deref(), Value::Array(_)) {
            self.warn("Array to string conversion")?;
        }
        Ok(())
    }

    /// `++`/`--` on a value with php's rules: ints overflow to float,
    /// numeric strings become numbers, non-numeric strings increment
    /// alphanumerically (deprecated), null++ is 1, bools/null-- warn and
    /// stay, arrays/objects are a `TypeError`.
    pub fn inc_dec(&mut self, v: &Value, inc: bool) -> Result<Value, Unwind> {
        let v = v.deref().into_owned();
        Ok(match v {
            Value::Int(i) => {
                if inc {
                    i.checked_add(1)
                        .map(Value::Int)
                        .unwrap_or(Value::Float(i as f64 + 1.0))
                } else {
                    i.checked_sub(1)
                        .map(Value::Int)
                        .unwrap_or(Value::Float(i as f64 - 1.0))
                }
            }
            Value::Float(f) => Value::Float(if inc { f + 1.0 } else { f - 1.0 }),
            Value::Null | Value::Uninit => {
                if inc {
                    Value::Int(1)
                } else {
                    self.warn(
                        "Decrement on type null has no effect, this will change in the next major version of PHP",
                    )?;
                    Value::Null
                }
            }
            Value::Bool(b) => {
                self.warn(&format!(
                    "{} on type bool has no effect, this will change in the next major version of PHP",
                    if inc { "Increment" } else { "Decrement" }
                ))?;
                Value::Bool(b)
            }
            Value::Str(ref s) => {
                if v.is_numeric() {
                    let n = v.to_number();
                    return self.inc_dec(&n, inc);
                }
                let s = s.clone();
                if s.is_empty() {
                    if inc {
                        self.deprecated("Increment on non-numeric string is deprecated, use str_increment() instead")?;
                        Value::string(b"1")
                    } else {
                        self.deprecated("Decrement on empty string is deprecated as non-numeric")?;
                        Value::Int(-1)
                    }
                } else if inc {
                    self.deprecated("Increment on non-numeric string is deprecated, use str_increment() instead")?;
                    Value::Str(Str::from_vec(str_increment(s.as_bytes())))
                } else {
                    self.deprecated("Decrement on non-numeric string has no effect and is deprecated")?;
                    Value::Str(s)
                }
            }
            other => {
                return Err(Unwind::type_error(format!(
                    "Cannot {} {}",
                    if inc { "increment" } else { "decrement" },
                    operand_type_name(&other)
                )))
            }
        })
    }

    /// `(kind) $v`.
    pub fn cast(&mut self, v: &Value, kind: CastKind) -> Result<Value, Unwind> {
        let v = v.deref().into_owned();
        Ok(match kind {
            CastKind::Int => match &v {
                Value::Object(o) => {
                    self.warn(&format!(
                        "Object of class {} could not be converted to int",
                        self.class_name_of(o)
                    ))?;
                    Value::Int(1)
                }
                _ => Value::Int(v.to_int()),
            },
            CastKind::Float => match &v {
                Value::Object(o) => {
                    self.warn(&format!(
                        "Object of class {} could not be converted to float",
                        self.class_name_of(o)
                    ))?;
                    Value::Float(1.0)
                }
                _ => Value::Float(v.to_float()),
            },
            CastKind::String => Value::Str(self.to_string(&v)?),
            CastKind::Bool => Value::Bool(v.to_bool()),
            CastKind::Array => match v {
                Value::Array(a) => Value::Array(a),
                Value::Null | Value::Uninit => Value::empty_array(),
                Value::Object(o) => Value::Array(self.object_to_array(&o)),
                other => {
                    let mut a = Array::new();
                    a.push(other);
                    Value::Array(a)
                }
            },
            CastKind::Object => match v {
                Value::Object(_) | Value::Closure(_) => v,
                Value::Array(a) => {
                    let std = self.class_by_name(b"stdClass").expect("stdClass is built in");
                    let o = self.instantiate(std);
                    for (k, val) in a.iter() {
                        let name = match k {
                            ArrayKey::Int(i) => i.to_string().into_bytes(),
                            ArrayKey::Str(s) => s.to_vec(),
                        };
                        o.dyn_set(&name, val.clone());
                    }
                    Value::Object(o)
                }
                Value::Null | Value::Uninit => {
                    let std = self.class_by_name(b"stdClass").expect("stdClass is built in");
                    Value::Object(self.instantiate(std))
                }
                other => {
                    let std = self.class_by_name(b"stdClass").expect("stdClass is built in");
                    let o = self.instantiate(std);
                    o.dyn_set(b"scalar", other);
                    Value::Object(o)
                }
            },
            CastKind::Unset => Value::Null,
        })
    }

    /// `(array) $object`: declared and dynamic properties in order, with
    /// php's mangled keys for private (`\0Class\0name`) and protected
    /// (`\0*\0name`) properties.
    pub fn object_to_array(&self, o: &Object) -> Array {
        let mut out = Array::new();
        // An initialized proxy casts as its real instance; an uninitialized
        // lazy object casts to nothing at all (php does not initialize it).
        let real = o.lazy_real();
        let o = real.as_ref().unwrap_or(o);
        // Each slot carries its own declaring class, which is what tells an
        // ancestor's private property apart from the subclass's of the same
        // name — the two keys differ only in the class between the NULs.
        let entries: Vec<(Vec<u8>, Value, rphp_value::Vis, Vec<u8>)> = o.with_data(|d| {
            d.props_in_order()
                .filter(|p| !p.value.is_uninit())
                .map(|p| {
                    let decl = p
                        .meta
                        .map(|m| m.decl_class_name.to_vec())
                        .unwrap_or_else(|| o.layout().class_name().to_vec());
                    (p.name.to_vec(), p.value.clone(), p.vis, decl)
                })
                .collect()
        });
        for (name, value, vis, decl) in entries {
            let key: Vec<u8> = match vis {
                rphp_value::Vis::Public => name,
                rphp_value::Vis::Protected => {
                    let mut k = b"\0*\0".to_vec();
                    k.extend_from_slice(&name);
                    k
                }
                rphp_value::Vis::Private => {
                    let mut k = vec![0u8];
                    k.extend_from_slice(&decl);
                    k.push(0);
                    k.extend_from_slice(&name);
                    k
                }
            };
            out.set(
                array_key(&Value::string(&key)).expect("string key"),
                value,
            );
        }
        out
    }

    /// `base[key]` read. Arrays index by normalized key (absent ⇒ warning +
    /// null); strings index by byte offset (negative allowed; out of range ⇒
    /// warning + ""). Indexing a scalar / null warns and yields null.
    pub fn array_get(&mut self, base: &Value, key: &Value) -> Result<Value, Unwind> {
        let base = base.deref();
        match &*base {
            Value::Array(a) => match array_key(key) {
                Some(k) => match a.get_deref(&k) {
                    Some(v) => Ok(v),
                    None => {
                        let msg = match &k {
                            ArrayKey::Int(i) => format!("Undefined array key {i}"),
                            ArrayKey::Str(s) => {
                                format!("Undefined array key \"{}\"", String::from_utf8_lossy(s))
                            }
                        };
                        self.warn(&msg)?;
                        Ok(Value::Null)
                    }
                },
                None => Err(Unwind::type_error(format!(
                    "Cannot access offset of type {} on array",
                    value_name(key)
                ))),
            },
            Value::Str(s) => self.string_offset(s, key),
            Value::Null | Value::Uninit | Value::Bool(_) | Value::Int(_) | Value::Float(_) => {
                let msg = format!("Trying to access array offset on {}", value_name(&base));
                self.warn(&msg)?;
                Ok(Value::Null)
            }
            Value::Object(o) => Err(Unwind::error(format!(
                "Cannot use object of type {} as array",
                self.class_name_of(o)
            ))),
            Value::Closure(_) => Err(Unwind::error(
                "Cannot use object of type Closure as array",
            )),
            Value::Resource(_) => Ok(Value::Null),
            Value::Ref(_) => unreachable!("deref'd above"),
        }
    }

    /// `base[key]` without warnings (`??`, `isset` chains): null when absent
    /// or not indexable.
    pub fn array_get_quiet(&self, base: &Value, key: &Value) -> Value {
        match &*base.deref() {
            Value::Array(a) => array_key(key)
                .and_then(|k| a.get_deref(&k))
                .unwrap_or(Value::Null),
            Value::Str(s) => {
                let len = s.len() as i64;
                if !matches!(key.deref().as_ref(), Value::Int(_) | Value::Float(_) | Value::Bool(_))
                    && !key.is_numeric()
                {
                    return Value::Null;
                }
                let mut i = key.to_int();
                if i < 0 {
                    i += len;
                }
                if i >= 0 && i < len {
                    Value::string(&s.as_bytes()[i as usize..i as usize + 1])
                } else {
                    Value::Null
                }
            }
            _ => Value::Null,
        }
    }

    /// `isset($base[$key])`.
    pub fn isset_elem(&self, base: &Value, key: &Value) -> bool {
        !matches!(self.array_get_quiet(base, key), Value::Null | Value::Uninit)
    }

    fn string_offset(&mut self, s: &Str, key: &Value) -> Result<Value, Unwind> {
        let len = s.len() as i64;
        let k = key.deref().into_owned();
        let requested = match &k {
            Value::Int(i) => *i,
            Value::Str(_) if k.is_numeric() => k.to_int(),
            Value::Str(_) => {
                return Err(Unwind::type_error(
                    "Cannot access offset of type string on string",
                ))
            }
            Value::Float(_) | Value::Bool(_) | Value::Null | Value::Uninit => {
                self.warn("String offset cast occurred")?;
                k.to_int()
            }
            other => {
                return Err(Unwind::type_error(format!(
                    "Cannot access offset of type {} on string",
                    value_name(other)
                )))
            }
        };
        let mut i = requested;
        if i < 0 {
            i += len; // PHP allows negative string offsets
        }
        if i >= 0 && i < len {
            Ok(Value::string(&s.as_bytes()[i as usize..i as usize + 1]))
        } else {
            self.warn(&format!("Uninitialized string offset {requested}"))?;
            Ok(Value::string(b""))
        }
    }

    /// `slot[key] = value` / `slot[] = value` on the container in `slot`,
    /// in place: null autovivifies to an array, `false` autovivifies with a
    /// deprecation, strings take a one-byte offset write, other scalars and
    /// objects are an `Error`. The diagnostic to emit afterwards (if any) is
    /// returned as a value so the mutation can happen under a reference-cell
    /// borrow.
    pub fn array_set_in(
        slot: &mut Value,
        key: Option<&Value>,
        value: Value,
        class_name: &dyn Fn(&Object) -> String,
    ) -> Result<SetNotice, Unwind> {
        let mut deprecated_false = false;
        if matches!(slot, Value::Bool(false)) {
            deprecated_false = true;
            *slot = Value::empty_array();
        }
        if matches!(slot, Value::Null | Value::Uninit) {
            *slot = Value::empty_array();
        }
        match slot {
            Value::Array(a) => match key {
                Some(k) => match array_key(k) {
                    Some(k) => a.set(k, value),
                    None => {
                        return Err(Unwind::type_error(format!(
                            "Cannot access offset of type {} on array",
                            value_name(k)
                        )))
                    }
                },
                None => a.push(value),
            },
            Value::Str(s) => {
                let Some(k) = key else {
                    return Err(Unwind::error("[] operator not supported for strings"));
                };
                let k = k.deref().into_owned();
                if !matches!(k, Value::Int(_)) && !k.is_numeric() {
                    return Err(Unwind::type_error(format!(
                        "Cannot access offset of type {} on string",
                        value_name(&k)
                    )));
                }
                let mut bytes = s.as_bytes().to_vec();
                let mut i = k.to_int();
                if i < 0 {
                    i += bytes.len() as i64;
                    if i < 0 {
                        return Err(Unwind::error(format!(
                            "Illegal string offset {}",
                            k.to_int()
                        )));
                    }
                }
                let v = value.to_php_bytes();
                if v.is_empty() {
                    return Err(Unwind::error("Cannot assign an empty string to a string offset"));
                }
                let i = i as usize;
                while bytes.len() <= i {
                    bytes.push(b' ');
                }
                bytes[i] = v[0];
                *slot = Value::Str(Str::from_vec(bytes));
                if v.len() > 1 {
                    return Ok(SetNotice::FirstByteOnly);
                }
            }
            Value::Object(o) => {
                return Err(Unwind::error(format!(
                    "Cannot use object of type {} as array",
                    class_name(o)
                )))
            }
            Value::Closure(_) => {
                return Err(Unwind::error("Cannot use object of type Closure as array"))
            }
            _ => return Err(Unwind::error("Cannot use a scalar value as an array")),
        }
        Ok(if deprecated_false {
            SetNotice::FalseToArray
        } else {
            SetNotice::None
        })
    }

    /// The `UnhandledMatchError` message for a subject value.
    pub fn match_error_message(&self, v: &Value) -> String {
        match &*v.deref() {
            Value::Null | Value::Uninit => "Unhandled match case NULL".to_string(),
            Value::Bool(true) => "Unhandled match case true".to_string(),
            Value::Bool(false) => "Unhandled match case false".to_string(),
            Value::Int(i) => format!("Unhandled match case {i}"),
            Value::Float(_) => format!("Unhandled match case {}", v.to_php_string()),
            Value::Str(s) => format!("Unhandled match case '{}'", String::from_utf8_lossy(s.as_bytes())),
            Value::Array(_) => "Unhandled match case of type array".to_string(),
            Value::Object(o) => format!("Unhandled match case of type {}", self.class_name_of(o)),
            Value::Closure(_) => "Unhandled match case of type Closure".to_string(),
            Value::Resource(_) => "Unhandled match case of type resource".to_string(),
            Value::Ref(_) => unreachable!("deref'd above"),
        }
    }
}

/// php's alphanumeric string increment (`"a"` → `"b"`, `"z"` → `"aa"`,
/// `"Az"` → `"Ba"`, `"a9"` → `"b0"`, `"Zz"` → `"AAa"`).
pub fn str_increment(s: &[u8]) -> Vec<u8> {
    let mut out = s.to_vec();
    let mut i = out.len();
    let mut carry_kind: Option<u8> = None;
    while i > 0 {
        i -= 1;
        let c = out[i];
        match c {
            b'a'..=b'y' | b'A'..=b'Y' | b'0'..=b'8' => {
                out[i] = c + 1;
                return out;
            }
            b'z' => {
                out[i] = b'a';
                carry_kind = Some(b'a');
            }
            b'Z' => {
                out[i] = b'A';
                carry_kind = Some(b'A');
            }
            b'9' => {
                out[i] = b'0';
                carry_kind = Some(b'1');
            }
            _ => return out,
        }
    }
    if let Some(k) = carry_kind {
        out.insert(0, k);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn string_increment_matches_php() {
        assert_eq!(str_increment(b"a"), b"b");
        assert_eq!(str_increment(b"z"), b"aa");
        assert_eq!(str_increment(b"Az"), b"Ba");
        assert_eq!(str_increment(b"a9"), b"b0");
        assert_eq!(str_increment(b"Zz"), b"AAa");
        assert_eq!(str_increment(b"zz"), b"aaa");
        assert_eq!(str_increment(b"a-"), b"a-");
    }
}
