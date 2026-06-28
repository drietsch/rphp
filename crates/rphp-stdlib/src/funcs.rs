//! Function-handling builtins that invoke a PHP callable through the host
//! (`ctx.call`). The callable is a function-name string for now; closures and
//! `[$obj, 'method']` forms arrive with the closure/object value types.
use rphp_value::Value;

use crate::{nf, Ctx, NativeError, NativeFn, NativeResult};

/// This extension's registry contribution (see `lib.rs`).
pub(crate) static FUNCTIONS: &[NativeFn] = &[
    nf!("call_user_func", 1, None, call_user_func),
    nf!("call_user_func_array", 2, Some(2), call_user_func_array),
];

/// `call_user_func($callable, ...$args)`: invoke `$callable` with the remaining
/// arguments and return its result.
pub(crate) fn call_user_func(ctx: &mut Ctx, args: &[Value]) -> NativeResult {
    ctx.call(&args[0], &args[1..])
}

/// `call_user_func_array($callable, $args)`: invoke `$callable` with the values
/// of the `$args` array (positional; string keys / named args not modelled yet).
pub(crate) fn call_user_func_array(ctx: &mut Ctx, args: &[Value]) -> NativeResult {
    let params: Vec<Value> = match &args[1] {
        Value::Array(a) => a.iter().map(|(_, v)| v.clone()).collect(),
        other => {
            return Err(NativeError::new(format!(
                "call_user_func_array(): Argument #2 ($args) must be of type array, {} given",
                other.type_name()
            )))
        }
    };
    ctx.call(&args[0], &params)
}
