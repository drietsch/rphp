//! Function-handling builtins that invoke a PHP callable through the
//! interpreter (`ctx.call_value`). The callable is a function-name string for now; closures and
//! `[$obj, 'method']` forms arrive with the closure/object value types.
use rphp_value::Value;

use rphp_runtime::{Ctx, NativeFn, NativeResult, nf, Unwind};

/// This extension's registry contribution (see `lib.rs`).
pub(crate) static FUNCTIONS: &[NativeFn] = &[
    nf!("call_user_func", 1, None, call_user_func),
    nf!("call_user_func_array", 2, Some(2), call_user_func_array),
];

/// `call_user_func($callable, ...$args)`: invoke `$callable` with the remaining
/// arguments and return its result.
pub(crate) fn call_user_func(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    ctx.call_value(&args[0], &args[1..])
}

/// `call_user_func_array($callable, $args)`: invoke `$callable` with the values
/// of the `$args` array (positional; string keys / named args not modelled yet).
pub(crate) fn call_user_func_array(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
    let params: Vec<Value> = match &args[1] {
        Value::Array(a) => a.iter().map(|(_, v)| v.clone()).collect(),
        other => {
            return Err(Unwind::type_error(format!(
                "call_user_func_array(): Argument #2 ($args) must be of type array, {} given",
                other.type_name()
            )))
        }
    };
    ctx.call_value(&args[0], &params)
}
