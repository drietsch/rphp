//! `IntlIterator`: the `Iterator` php's intl hands back for a list ICU
//! produced (the timezone enumerations, the keyword values).

use rphp_runtime::{Ctx, Interp, NativeResult, Registry, Unwind};
use rphp_value::{Array, Object, Payload, Value};

use crate::generated;
use crate::shape::{register_class, MethodImpl};

/// The list and where the cursor sits.
pub struct IterState {
    items: Vec<Value>,
    pos: usize,
}

/// A new `IntlIterator` over an array's values.
pub fn new_object(ctx: &mut Ctx, items: Array) -> NativeResult {
    let cid = ctx.lookup_class_or_error(b"IntlIterator")?;
    let obj = ctx.instantiate(cid);
    obj.set_payload(Payload::Native(Box::new(IterState { items: items.values().cloned().collect(), pos: 0 })));
    Ok(Value::Object(obj))
}

fn with_state<R>(o: &Object, f: impl FnOnce(&mut IterState) -> R) -> Option<R> {
    o.with_payload::<IterState, _>(f)
}

fn this(o: Option<&Object>) -> Result<&Object, Unwind> {
    o.ok_or_else(|| Unwind::error("Non-static method called statically"))
}

fn payload_clone(_: &mut Interp, src: &Object, dst: &Object) -> Result<(), Unwind> {
    if let Some(copy) = src.with_payload::<IterState, _>(|s| IterState { items: s.items.clone(), pos: s.pos }) {
        dst.set_payload(Payload::Native(Box::new(copy)));
    }
    Ok(())
}

fn current(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    Ok(with_state(o, |s| s.items.get(s.pos).cloned()).flatten().unwrap_or(Value::Null))
}

fn key(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    Ok(with_state(o, |s| if s.pos < s.items.len() { Value::Int(s.pos as i64) } else { Value::Null }).unwrap_or(Value::Null))
}

fn next(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    with_state(o, |s| s.pos += 1);
    Ok(Value::Null)
}

fn rewind(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    with_state(o, |s| s.pos = 0);
    Ok(Value::Null)
}

fn valid(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let o = this(o)?;
    Ok(Value::Bool(with_state(o, |s| s.pos < s.items.len()).unwrap_or(false)))
}

static METHODS: &[MethodImpl] = &[("current", current), ("key", key), ("next", next), ("rewind", rewind), ("valid", valid)];

pub fn register(r: &mut Registry) {
    register_class(r, &generated::classes::INTLITERATOR, METHODS, |b| b.payload_clone(payload_clone));
}
