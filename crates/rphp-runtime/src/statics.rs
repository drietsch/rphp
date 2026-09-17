//! Static properties and class constants (plan E6).
//!
//! **This module owns class-level storage.** `exec.rs` resolves the class
//! reference and the member name, then calls in here.
//!
//! Both kinds are *lazily initialized*: linking a class never runs user code,
//! so a declared initializer is kept as a [`PropDefault`](crate::PropDefault)
//! and evaluated on first access, in the declaring class's scope so `self::`
//! and `static::` resolve correctly.
//!
//! What lands here as E6 fills in:
//!
//! * `A::$p` read/write/ref/isset over the shared
//!   [`StaticPropInfo::cell`](crate::StaticPropInfo::cell) — a subclass that
//!   does not redeclare shares the parent's cell, so `B::$n` and `A::$n` are
//!   one location;
//! * class constants with **cycle detection**: an initializer that refers to
//!   itself must raise php's "Cannot declare self-referencing constant"
//!   rather than recurse (see [`ConstState`](crate::ConstState));
//! * visibility of both kinds relative to the calling scope, and the
//!   `Access to undeclared static property` / `Undefined constant` errors;
//! * **late static binding**: `static::` resolves to the called scope, which
//!   the frame carries, so `static::$p`, `static::CONST` and `new static`
//!   pick the runtime class rather than the lexical one;
//! * `parent::`/`self::` forwarding, which keeps the *called* scope.

use std::cell::RefCell;
use std::rc::Rc;

use rphp_value::Value;

use crate::class::ConstState;
use crate::registry::Unwind;
use crate::Interp;

impl Interp {
    /// The shared storage cell of `Class::$name`, initializing it on first
    /// access. `scope` is the calling class, for the visibility check.
    ///
    /// E6 implements this; until then every static property is undeclared.
    pub(crate) fn static_prop_cell(
        &mut self,
        cid: u32,
        name: &[u8],
        _scope: Option<u32>,
    ) -> Result<Rc<RefCell<Value>>, Unwind> {
        Err(Unwind::error(format!(
            "Access to undeclared static property {}::${}",
            self.classes[cid as usize].name_str(),
            String::from_utf8_lossy(name)
        )))
    }

    /// The value of `Class::NAME`, evaluating the initializer on first use.
    ///
    /// E6 implements this; until then only `::class` (handled in `exec.rs`)
    /// resolves.
    pub(crate) fn class_const(
        &mut self,
        cid: u32,
        name: &[u8],
        _scope: Option<u32>,
    ) -> Result<Value, Unwind> {
        let _ = ConstState::Evaluating;
        Err(Unwind::error(format!(
            "Undefined constant {}::{}",
            self.classes[cid as usize].name_str(),
            String::from_utf8_lossy(name)
        )))
    }
}
