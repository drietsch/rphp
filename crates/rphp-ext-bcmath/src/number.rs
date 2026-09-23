//! php 8.4's `final readonly class BcMath\Number implements Stringable`:
//! an immutable decimal with its own scale. Its two properties, `value`
//! and `scale`, are computed from the payload (php declares them
//! `@virtual`) and refuse every write. Results pick their scale
//! automatically (`add`: the larger scale, `mul`: the sum, `div`: ten more
//! digits, then trimmed) unless a `$scale` is given, and instances take
//! part in `+ - * / % **` and in comparisons with ints, numeric strings
//! and each other through the runtime's operator hooks
//! ([`rphp_runtime::NativeOperators`]).

use rphp_runtime::{
    nm, ClassFlags, Ctx, Interp, NativeOperators, NativeProps, NativeResult, Registry, Unwind,
};
use rphp_value::{Array, ArrayKey, CastTarget, Object, Payload, Str, Value};

use crate::num::{self, BcNum};
use crate::{check_scale, not_well_formed, INT_MAX};

const CLASS: &str = "BcMath\\Number";

/// php's `BC_MATH_NUMBER_EXPAND_SCALE`: the extra digits a division, a
/// negative power or a square root computes before trimming.
const EXPAND_SCALE: usize = 10;

/// The payload: the number and its scale (which may exceed the digits
/// the number keeps: `new Number("1.50")` has scale 2 and one stored
/// fractional digit).
#[derive(Clone)]
struct NumState {
    num: BcNum,
    scale: usize,
}

pub(crate) fn register(r: &mut Registry) {
    r.class(CLASS)
        .flags(ClassFlags::FINAL | ClassFlags::READONLY)
        .implements(&["Stringable"])
        .method("__construct", nm!(1, Some(1), construct))
        .method("add", nm!(1, Some(2), add))
        .method("sub", nm!(1, Some(2), sub))
        .method("mul", nm!(1, Some(2), mul))
        .method("div", nm!(1, Some(2), div))
        .method("mod", nm!(1, Some(2), modulo))
        .method("divmod", nm!(1, Some(2), divmod))
        .method("powmod", nm!(2, Some(3), powmod))
        .method("pow", nm!(1, Some(2), pow))
        .method("sqrt", nm!(0, Some(1), sqrt))
        .method("floor", nm!(0, Some(0), floor))
        .method("ceil", nm!(0, Some(0), ceil))
        .method("round", nm!(0, Some(2), round))
        .method("compare", nm!(1, Some(2), compare))
        .method("__toString", nm!(0, Some(0), to_string))
        .method("__serialize", nm!(0, Some(0), serialize))
        .method("__unserialize", nm!(1, Some(1), unserialize))
        .native_props(NativeProps {
            names: &["value", "scale"],
            get: prop_get,
            set: prop_set,
            isset: Some(prop_isset),
            unset: Some(prop_unset),
            list: Some(prop_table),
            debug: Some(prop_table),
            cast: Some(prop_table),
        })
        .payload_clone(payload_clone)
        .operators(NativeOperators {
            binary: operator,
            compare: operand_compare,
            compare_notices: Some(compare_notices),
        })
        .finish();
}

// ---- the payload ------------------------------------------------------------

fn state(o: &Object) -> Option<NumState> {
    o.with_payload::<NumState, _>(|s| s.clone())
}

/// The state of `$this`; php's object always has one once constructed.
fn this_state(this: Option<&Object>) -> Result<NumState, Unwind> {
    this.and_then(state).ok_or_else(|| Unwind::error("BcMath\\Number is not initialized"))
}

fn install(o: &Object, st: NumState) {
    o.set_payload(Payload::Native(Box::new(st)));
    o.set_cast_handler(cast_handler);
}

/// A new instance holding `num` at `scale`.
fn new_number(ctx: &mut Interp, num: BcNum, scale: usize) -> Result<Value, Unwind> {
    let cid = ctx
        .class_by_name(CLASS.as_bytes())
        .ok_or_else(|| Unwind::error("Class \"BcMath\\Number\" not found"))?;
    let o = ctx.instantiate(cid);
    install(&o, NumState { num, scale });
    Ok(Value::Object(o))
}

/// php's `cast_object`: truthiness is "not zero"; the other casts are the
/// engine's (`__toString`, the int/float warnings).
fn cast_handler(o: &Object, target: CastTarget) -> Option<Value> {
    match target {
        CastTarget::Bool => state(o).map(|s| Value::Bool(!s.num.is_zero())),
        _ => None,
    }
}

fn payload_clone(_: &mut Interp, src: &Object, dst: &Object) -> Result<(), Unwind> {
    if let Some(st) = state(src) {
        install(dst, st);
    }
    Ok(())
}

fn value_str(st: &NumState) -> Value {
    Value::Str(Str::from_vec(st.num.to_str(st.scale)))
}

// ---- properties --------------------------------------------------------------

fn prop_get(_: &mut Interp, o: &Object, name: &[u8]) -> Option<Result<Value, Unwind>> {
    let st = state(o)?;
    match name {
        b"value" => Some(Ok(value_str(&st))),
        b"scale" => Some(Ok(Value::Int(st.scale as i64))),
        _ => None,
    }
}

fn prop_set(_: &mut Interp, _: &Object, name: &[u8], _: Value) -> Option<Result<(), Unwind>> {
    let name = String::from_utf8_lossy(name);
    Some(Err(Unwind::error(match &*name {
        "value" | "scale" => format!("Cannot modify readonly property {CLASS}::${name}"),
        _ => format!("Cannot create dynamic property {CLASS}::${name}"),
    })))
}

fn prop_isset(_: &mut Interp, _: &Object, name: &[u8]) -> Option<bool> {
    matches!(name, b"value" | b"scale").then_some(true)
}

fn prop_unset(_: &mut Interp, _: &Object, name: &[u8]) -> Option<Result<(), Unwind>> {
    matches!(name, b"value" | b"scale").then(|| {
        Err(Unwind::error(format!(
            "Cannot unset readonly property {CLASS}::${}",
            String::from_utf8_lossy(name)
        )))
    })
}

/// php's `get_properties_for`: `value` and `scale`.
fn prop_table(_: &mut Interp, o: &Object) -> Vec<(ArrayKey, Value)> {
    let Some(st) = state(o) else {
        return Vec::new();
    };
    vec![
        (ArrayKey::str(b"value"), value_str(&st)),
        (ArrayKey::str(b"scale"), Value::Int(st.scale as i64)),
    ]
}

// ---- operands -----------------------------------------------------------------

/// A `Number|string|int` operand before it is read as a number.
enum Operand {
    Num(NumState),
    Str(Vec<u8>),
    Long(i64),
}

impl Operand {
    /// `bc_num_from_obj_or_str_or_long`: the number and its full scale;
    /// `None` for a string that is not a number.
    fn to_num(&self) -> Option<(BcNum, usize)> {
        match self {
            Operand::Num(st) => Some((st.num.clone(), st.scale)),
            Operand::Str(s) => BcNum::parse(s, 0, true),
            Operand::Long(l) => Some((BcNum::from_i64(*l), 0)),
        }
    }
}

fn number_state(v: &Value) -> Option<NumState> {
    match v {
        Value::Object(o) => state(o),
        _ => None,
    }
}

/// Whether `v` is a `Number` (the class is final: only its instances
/// carry the payload).
fn is_number(v: &Value) -> bool {
    matches!(v, Value::Object(o) if o.with_payload::<NumState, _>(|_| ()).is_some())
}

/// A finite float in `i64` range (`ZEND_DOUBLE_FITS_LONG`, NaN excluded).
fn fits_long(f: f64) -> bool {
    // 2⁶³, exactly.
    const LIMIT: f64 = i64::MAX as f64;
    f.is_finite() && (-LIMIT..LIMIT).contains(&f)
}

/// A float as php's weak `int` parameter parsing takes it
/// (`zend_parse_arg_long_weak`): whole and in range, or fractional with
/// php 8.1's deprecation; `None` when it cannot be an int.
fn float_to_long(ctx: &mut Interp, f: f64) -> Result<Option<i64>, Unwind> {
    if !fits_long(f) {
        return Ok(None);
    }
    if f.fract() != 0.0 {
        ctx.deprecated(&format!(
            "Implicit conversion from float {} to int loses precision",
            Value::Float(f).to_php_string()
        ))?;
    }
    Ok(Some(f as i64))
}

/// A method's `Number|string|int` argument (php's
/// `BCMATH_PARAM_NUMBER_OR_STR_OR_LONG`, weak mode).
fn method_operand(
    ctx: &mut Interp,
    v: &Value,
    method: &str,
    argno: usize,
    pname: &str,
) -> Result<Operand, Unwind> {
    let v = v.deref().into_owned();
    if let Some(st) = number_state(&v) {
        return Ok(Operand::Num(st));
    }
    Ok(match &v {
        Value::Int(i) => Operand::Long(*i),
        Value::Str(s) => Operand::Str(s.as_bytes().to_vec()),
        Value::Null | Value::Uninit => {
            ctx.deprecated(&format!(
                "{CLASS}::{method}(): Passing null to parameter #{argno} (${pname}) of type {CLASS}|string|int is deprecated"
            ))?;
            Operand::Long(0)
        }
        Value::Bool(b) => Operand::Long(i64::from(*b)),
        Value::Float(f) => match float_to_long(ctx, *f)? {
            Some(l) => Operand::Long(l),
            None => {
                if f.is_nan() {
                    ctx.warn("unexpected NAN value was coerced to string")?;
                }
                Operand::Str(v.to_php_bytes())
            }
        },
        Value::Object(o)
            if ctx.class_of(o).magic.contains(rphp_runtime::MagicFlags::TOSTRING) =>
        {
            let o = o.clone();
            Operand::Str(ctx.object_to_string(&o)?.as_bytes().to_vec())
        }
        other => {
            return Err(Unwind::type_error(format!(
                "{CLASS}::{method}(): Argument #{argno} (${pname}) must be of type int, string, or {CLASS}, {} given",
                rphp_runtime::value_name(other)
            )))
        }
    })
}

/// `bc_num_from_obj_or_str_or_long_with_err`.
fn operand_num(op: &Operand, method: &str, argno: usize, pname: &str) -> Result<(BcNum, usize), Unwind> {
    let (n, scale) = op.to_num().ok_or_else(|| not_well_formed(&format!("{CLASS}::{method}"), argno, pname))?;
    if scale as i64 > INT_MAX {
        return Err(Unwind::value_error(format!(
            "{CLASS}::{method}(): Argument #{argno} (${pname}) must be between 0 and {INT_MAX}"
        )));
    }
    Ok((n, scale))
}

/// A method's optional `?int $scale`: `None` when absent or null.
///
/// These methods parse their object arguments themselves (see
/// [`method_operand`]), so an object here is refused here too.
fn method_scale(args: &[Value], idx: usize, method: &str) -> Result<Option<usize>, Unwind> {
    match args.get(idx).map(|v| v.deref().into_owned()) {
        None | Some(Value::Null) => Ok(None),
        Some(v @ (Value::Object(_) | Value::Closure(_))) => Err(Unwind::type_error(format!(
            "{CLASS}::{method}(): Argument #{} ($scale) must be of type ?int, {} given",
            idx + 1,
            rphp_runtime::value_name(&v)
        ))),
        Some(v) => check_scale(v.to_int(), &format!("{CLASS}::{method}"), idx + 1, "scale").map(Some),
    }
}

// ---- the arithmetic with its scale rules ------------------------------------------

/// The arithmetic operators and methods.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Calc {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Pow,
}

fn too_large_scale() -> Unwind {
    Unwind::value_error("scale of the result is too large")
}

/// php's `bcmath_number_*_internal`: `n1 OP n2` at `scale`, or at the
/// automatic scale when `scale` is `None`. `pow_arg` names the method's
/// exponent argument for the pow errors (`None` for the operator).
fn calc(
    op: Calc,
    n1: &BcNum,
    s1: usize,
    n2: &BcNum,
    s2: usize,
    scale: Option<usize>,
    pow_method: Option<&str>,
) -> Result<(BcNum, usize), Unwind> {
    let auto = scale.is_none();
    match op {
        Calc::Add | Calc::Sub => {
            let scale = scale.unwrap_or(s1.max(s2));
            let mut r = if op == Calc::Add { num::add(n1, n2, scale) } else { num::sub(n1, n2, scale) };
            cut(&mut r, scale);
            r.rm_trailing_zeros();
            Ok((r, scale))
        }
        Calc::Mul => {
            let scale = match scale {
                Some(s) => s,
                None => {
                    let s = s1 + s2;
                    if s as i64 > INT_MAX {
                        return Err(too_large_scale());
                    }
                    s
                }
            };
            let mut r = num::multiply(n1, n2, scale);
            cut(&mut r, scale);
            r.rm_trailing_zeros();
            Ok((r, scale))
        }
        Calc::Div => {
            let mut scale = match scale {
                Some(s) => s,
                None => {
                    let s = s1 + EXPAND_SCALE;
                    if s as i64 > INT_MAX {
                        return Err(too_large_scale());
                    }
                    s
                }
            };
            let mut r = num::divide(n1, n2, scale).ok_or_else(|| Unwind::division_by_zero("Division by zero"))?;
            r.rm_trailing_zeros();
            if auto {
                scale -= (scale - r.scale).min(EXPAND_SCALE);
            }
            Ok((r, scale))
        }
        Calc::Mod => {
            let scale = scale.unwrap_or(s1.max(s2));
            let mut r = num::modulo(n1, n2, scale).ok_or_else(|| Unwind::division_by_zero("Modulo by zero"))?;
            r.rm_trailing_zeros();
            Ok((r, scale))
        }
        Calc::Pow => pow_calc(n1, s1, n2, scale, pow_method),
    }
}

/// `(*ret)->n_scale = MIN(scale, (*ret)->n_scale)`.
fn cut(r: &mut BcNum, scale: usize) {
    if r.scale > scale {
        r.scale = scale;
        r.d.truncate(r.len + scale);
    }
}

/// php's `bcmath_number_pow_internal`.
fn pow_calc(
    n1: &BcNum,
    s1: usize,
    n2: &BcNum,
    scale: Option<usize>,
    method: Option<&str>,
) -> Result<(BcNum, usize), Unwind> {
    let arg_error = |what: &str| match method {
        None => Unwind::value_error(format!("exponent {what}")),
        Some(m) => Unwind::value_error(format!("{CLASS}::{m}(): Argument #1 ($exponent) exponent {what}")),
    };
    if n2.scale != 0 {
        return Err(arg_error("cannot have a fractional part"));
    }
    let exponent = n2.to_long();
    let mut expand = false;
    let mut scale = match scale {
        Some(s) => s,
        None => {
            if exponent > 0 {
                match s1.checked_mul(exponent as usize) {
                    Some(s) if s as i64 <= INT_MAX => s,
                    _ => return Err(too_large_scale()),
                }
            } else if exponent < 0 {
                let s = s1 + EXPAND_SCALE;
                if s as i64 > INT_MAX {
                    return Err(too_large_scale());
                }
                expand = true;
                s
            } else {
                0
            }
        }
    };
    if exponent == 0 && !n2.is_zero() {
        return Err(arg_error("is too large"));
    }
    let func = method.map(|m| format!("{CLASS}::{m}"));
    let mut r = num::raise(n1, exponent, scale)
        .map_err(|e| crate::raise_error(e, func.as_deref().map(|f| (f, 1, "exponent"))))?;
    r.rm_trailing_zeros();
    if expand {
        scale -= (scale - r.scale).min(EXPAND_SCALE);
    }
    Ok((r, scale))
}

// ---- the methods ----------------------------------------------------------------

/// `__construct(string|int $num)`
fn construct(_: &mut Ctx, this: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let Some(o) = this else {
        return Ok(Value::Null);
    };
    if state(o).is_some() {
        return Err(Unwind::error(format!("Cannot modify readonly property {CLASS}::$value")));
    }
    let v = args[0].deref().into_owned();
    let op = match &v {
        Value::Int(i) => Operand::Long(*i),
        _ => Operand::Str(v.to_php_bytes()),
    };
    let (num, scale) = operand_num(&op, "__construct", 1, "num")?;
    install(o, NumState { num, scale });
    Ok(Value::Null)
}

/// `add`/`sub`/`mul`/`div`/`mod`/`pow`: `(Number|string|int $num, ?int $scale = null)`.
fn calc_method(ctx: &mut Ctx, this: Option<&Object>, args: &mut [Value], op: Calc, method: &str) -> NativeResult {
    let pname = if op == Calc::Pow { "exponent" } else { "num" };
    let operand = method_operand(ctx, &args[0], method, 1, pname)?;
    let (n2, s2) = operand_num(&operand, method, 1, pname)?;
    let scale = method_scale(args, 1, method)?;
    let st = this_state(this)?;
    let (r, scale) = calc(op, &st.num, st.scale, &n2, s2, scale, Some(method))?;
    new_number(ctx, r, scale)
}

fn add(ctx: &mut Ctx, this: Option<&Object>, args: &mut [Value]) -> NativeResult {
    calc_method(ctx, this, args, Calc::Add, "add")
}

fn sub(ctx: &mut Ctx, this: Option<&Object>, args: &mut [Value]) -> NativeResult {
    calc_method(ctx, this, args, Calc::Sub, "sub")
}

fn mul(ctx: &mut Ctx, this: Option<&Object>, args: &mut [Value]) -> NativeResult {
    calc_method(ctx, this, args, Calc::Mul, "mul")
}

fn div(ctx: &mut Ctx, this: Option<&Object>, args: &mut [Value]) -> NativeResult {
    calc_method(ctx, this, args, Calc::Div, "div")
}

fn modulo(ctx: &mut Ctx, this: Option<&Object>, args: &mut [Value]) -> NativeResult {
    calc_method(ctx, this, args, Calc::Mod, "mod")
}

fn pow(ctx: &mut Ctx, this: Option<&Object>, args: &mut [Value]) -> NativeResult {
    calc_method(ctx, this, args, Calc::Pow, "pow")
}

/// `divmod(Number|string|int $num, ?int $scale = null): array`
fn divmod(ctx: &mut Ctx, this: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let operand = method_operand(ctx, &args[0], "divmod", 1, "num")?;
    let (n2, s2) = operand_num(&operand, "divmod", 1, "num")?;
    let scale = method_scale(args, 1, "divmod")?;
    let st = this_state(this)?;
    let scale = scale.unwrap_or(st.scale.max(s2));
    let (mut q, mut r) =
        num::divmod(&st.num, &n2, scale).ok_or_else(|| Unwind::division_by_zero("Division by zero"))?;
    q.rm_trailing_zeros();
    r.rm_trailing_zeros();
    let mut out = Array::new();
    out.push(new_number(ctx, q, 0)?);
    out.push(new_number(ctx, r, scale)?);
    Ok(Value::Array(out))
}

/// `powmod(Number|string|int $exponent, Number|string|int $modulus, ?int $scale = null): Number`
fn powmod(ctx: &mut Ctx, this: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let e = method_operand(ctx, &args[0], "powmod", 1, "exponent")?;
    let m = method_operand(ctx, &args[1], "powmod", 2, "modulus")?;
    let (expo, _) = operand_num(&e, "powmod", 1, "exponent")?;
    let (modulus, _) = operand_num(&m, "powmod", 2, "modulus")?;
    let scale = method_scale(args, 2, "powmod")?.unwrap_or(0);
    let st = this_state(this)?;
    let mut r = num::raisemod(&st.num, &expo, &modulus, scale).map_err(|e| {
        crate::raisemod_error(e, &format!("{CLASS}::powmod"), |n| ["exponent", "modulus"][n - 1], 1)
    })?;
    r.rm_trailing_zeros();
    new_number(ctx, r, scale)
}

/// `sqrt(?int $scale = null): Number`
fn sqrt(ctx: &mut Ctx, this: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let given = match args.first().map(|v| v.deref().into_owned()) {
        None | Some(Value::Null) => None,
        Some(v) => Some(check_scale(v.to_int(), &format!("{CLASS}::sqrt"), 1, "scale")?),
    };
    let st = this_state(this)?;
    let mut scale = match given {
        Some(s) => s,
        None => {
            let s = st.scale + EXPAND_SCALE;
            if s as i64 > INT_MAX {
                return Err(too_large_scale());
            }
            s
        }
    };
    let mut r = num::sqrt(&st.num, scale)
        .ok_or_else(|| Unwind::value_error("Base number must be greater than or equal to 0"))?;
    cut(&mut r, scale);
    r.rm_trailing_zeros();
    if given.is_none() {
        scale -= (scale - r.scale).min(EXPAND_SCALE);
    }
    new_number(ctx, r, scale)
}

fn floor(ctx: &mut Ctx, this: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let st = this_state(this)?;
    new_number(ctx, num::floor_or_ceil(&st.num, true), 0)
}

fn ceil(ctx: &mut Ctx, this: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let st = this_state(this)?;
    new_number(ctx, num::floor_or_ceil(&st.num, false), 0)
}

/// `round(int $precision = 0, RoundingMode $mode = RoundingMode::HalfAwayFromZero): Number`
fn round(ctx: &mut Ctx, this: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let precision = args.first().map_or(0, |v| v.deref().to_int());
    let method = format!("{CLASS}::round");
    let mode = crate::rounding_mode(ctx, args.get(1), &method, 2)?;
    crate::check_precision(precision, &method, 1)?;
    let st = this_state(this)?;
    let (mut r, scale) = num::round(&st.num, precision, mode);
    r.rm_trailing_zeros();
    new_number(ctx, r, scale)
}

/// `compare(Number|string|int $num, ?int $scale = null): int`
fn compare(ctx: &mut Ctx, this: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let operand = method_operand(ctx, &args[0], "compare", 1, "num")?;
    let (n2, _) = operand_num(&operand, "compare", 1, "num")?;
    let scale = method_scale(args, 1, "compare")?;
    let st = this_state(this)?;
    let scale = scale.unwrap_or(st.num.scale.max(n2.scale));
    Ok(Value::Int(num::compare(&st.num, &n2, scale)))
}

fn to_string(_: &mut Ctx, this: Option<&Object>, _: &mut [Value]) -> NativeResult {
    Ok(value_str(&this_state(this)?))
}

/// `__serialize(): array` — `['value' => …]`.
fn serialize(_: &mut Ctx, this: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let st = this_state(this)?;
    let mut out = Array::new();
    out.set(ArrayKey::str(b"value"), value_str(&st));
    Ok(Value::Array(out))
}

/// `__unserialize(array $data): void`
fn unserialize(_: &mut Ctx, this: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let invalid = || Unwind::exception("Exception", format!("Invalid serialization data for {CLASS} object"));
    let Some(o) = this else {
        return Ok(Value::Null);
    };
    let data = args[0].deref().into_owned();
    let value = match &data {
        Value::Array(a) => a.get_deref(&ArrayKey::str(b"value")),
        _ => None,
    };
    let s = match value {
        Some(Value::Str(s)) if !s.is_empty() => s,
        _ => return Err(invalid()),
    };
    if state(o).is_some() {
        return Err(Unwind::error(format!("Cannot modify readonly property {CLASS}::$value")));
    }
    let (num, scale) = BcNum::parse(s.as_bytes(), 0, true).ok_or_else(invalid)?;
    install(o, NumState { num, scale });
    Ok(Value::Null)
}

// ---- operators --------------------------------------------------------------------

/// php's `bcmath_number_parse_num` (weak mode): `None` when the value
/// cannot be an operand at all.
fn op_operand(ctx: &mut Interp, v: &Value) -> Result<Option<Operand>, Unwind> {
    if let Some(st) = number_state(v) {
        return Ok(Some(Operand::Num(st)));
    }
    Ok(match v {
        Value::Int(i) => Some(Operand::Long(*i)),
        Value::Str(s) => Some(Operand::Str(s.as_bytes().to_vec())),
        Value::Bool(b) => Some(Operand::Long(i64::from(*b))),
        Value::Float(f) => float_to_long(ctx, *f)?.map(Operand::Long),
        _ => None,
    })
}

/// The `do_operation` handler: `+ - * / % **` with a `Number` on either
/// side; anything else declines.
fn operator(
    ctx: &mut Interp,
    op: rphp_runtime::AssignOpKind,
    a: &Value,
    b: &Value,
) -> Result<Option<Value>, Unwind> {
    use rphp_runtime::AssignOpKind as K;
    let calc_op = match op {
        K::Add => Calc::Add,
        K::Sub => Calc::Sub,
        K::Mul => Calc::Mul,
        K::Div => Calc::Div,
        K::Mod => Calc::Mod,
        K::Pow => Calc::Pow,
        _ => return Ok(None),
    };
    let Some(l) = op_operand(ctx, a)? else {
        return Ok(None);
    };
    let Some(r) = op_operand(ctx, b)? else {
        return Ok(None);
    };
    let (n1, s1) = l
        .to_num()
        .ok_or_else(|| Unwind::value_error(format!("Left string operand cannot be converted to {CLASS}")))?;
    let (n2, s2) = r
        .to_num()
        .ok_or_else(|| Unwind::value_error(format!("Right string operand cannot be converted to {CLASS}")))?;
    if s1 as i64 > INT_MAX || s2 as i64 > INT_MAX {
        return Err(Unwind::value_error(format!("scale must be between 0 and {INT_MAX}")));
    }
    let (res, scale) = calc(calc_op, &n1, s1, &n2, s2, None, None)?;
    new_number(ctx, res, scale).map(Some)
}

/// What the `compare` handler says while it reads its operands, left to
/// right: a fractional float becomes an int with php's deprecation; an
/// operand it cannot take ends the reading.
fn compare_notices(ctx: &mut Interp, a: &Value, b: &Value) -> Result<(), Unwind> {
    for v in [a, b] {
        match v {
            Value::Int(_) | Value::Str(_) | Value::Bool(_) => {}
            Value::Object(_) if is_number(v) => {}
            Value::Float(f) => {
                if float_to_long(ctx, *f)?.is_none() {
                    return Ok(());
                }
            }
            _ => return Ok(()),
        }
    }
    Ok(())
}

/// The `compare` handler: `Number` against a `Number`, an int or a
/// numeric string (a bool or a float as the int it is); anything else is
/// uncomparable (`1`). The deprecation a fractional float earns comes
/// from [`compare_notices`] (a value-level comparison cannot raise it).
fn operand_compare(a: &Value, b: &Value) -> i64 {
    fn parse(v: &Value) -> Option<Operand> {
        match v {
            Value::Object(o) => state(o).map(Operand::Num),
            Value::Int(i) => Some(Operand::Long(*i)),
            Value::Str(s) => Some(Operand::Str(s.as_bytes().to_vec())),
            Value::Bool(b) => Some(Operand::Long(i64::from(*b))),
            Value::Float(f) if fits_long(*f) => Some(Operand::Long(*f as i64)),
            _ => None,
        }
    }
    let (Some(l), Some(r)) = (parse(a), parse(b)) else {
        return 1;
    };
    let (Some((n1, _)), Some((n2, _))) = (l.to_num(), r.to_num()) else {
        return 1;
    };
    num::compare(&n1, &n2, n1.scale.max(n2.scale))
}
