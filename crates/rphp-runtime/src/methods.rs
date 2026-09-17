//! Method dispatch and callables (plan E6).
//!
//! **This module owns how a method call and a callable value are resolved.**
//!
//! What lands here as E6 fills in:
//!
//! * `__call` / `__callStatic`, consulted when a method is missing or not
//!   visible from the calling scope, and `__invoke`, which makes an object
//!   callable;
//! * the static/non-static errors (`Non-static method C::m() cannot be
//!   called statically`) and visibility checks relative to the *calling*
//!   scope, not the object's class;
//! * `resolve_callable()` — the one shared path for every callable spelling:
//!   `'f'`, `'A::m'`, `[$obj, 'm']`, `['A', 'm']`, a `Closure`, an object
//!   with `__invoke`, and a first-class callable — so `array_map`,
//!   `call_user_func`, `usort` and direct `$f()` all agree;
//! * first-class callable syntax (`strlen(...)`, `$o->m(...)`, `A::m(...)`),
//!   which builds a `Closure` from the pending-call record that
//!   [`Interp::make_callable_closure`] receives;
//! * the `Closure` semantics behind the builtin class: `bind`/`bindTo`/`call`
//!   (rebinding `$this` and the scope), `fromCallable`, static closures.

use rphp_bytecode::Op;
use rphp_value::Value;

use crate::registry::Unwind;
use crate::unit::FuncRt;
use crate::Interp;

impl Interp {
    /// `f(...)` / `$o->m(...)` / `A::m(...)` — build a `Closure` from a
    /// first-class callable record.
    ///
    /// E6 implements this; until then it is php's "not supported" fatal.
    pub(crate) fn make_callable_closure(
        &mut self,
        _func: &std::rc::Rc<FuncRt>,
        _base: usize,
        _op: &Op,
    ) -> Result<Value, Unwind> {
        Err(Unwind::error(
            "first-class callable syntax is not supported yet",
        ))
    }
}
