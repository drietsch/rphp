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
//!
//! php details the code here depends on, all confirmed against 8.5:
//!
//! * a **private** constant is not inherited (`B::X` where `X` is private in
//!   `A` is *undefined*, not inaccessible), while a private static property
//!   is inherited into the subclass's table and merely inaccessible;
//! * the access errors name the class the member was **referenced** through
//!   (`E::$p`), the type errors name the class that **declared** it
//!   (`F::$n of type int`);
//! * an *initializer* is type-checked strictly (`public static int $n = Q;`
//!   with `Q = "5"` is a `TypeError` even in weak mode), while an
//!   *assignment* is coerced under the assigning file's `strict_types`.

use std::cell::RefCell;
use std::rc::Rc;

use rphp_bytecode::{TypeDecl, Visibility};
use rphp_value::{PhpRef, Value};

use crate::class::{ClassConst, ConstState, PropDefault};
use crate::exec::vis_word;
use crate::registry::Unwind;
use crate::types::Coerced;
use crate::Interp;

impl Interp {
    /// The shared storage cell of `Class::$name`, initializing it on first
    /// access. `scope` is the calling class, for the visibility check.
    ///
    /// The cell may hold `Value::Uninit` (a typed property with no
    /// initializer) and may hold a `Value::Ref` once a reference to it has
    /// been taken; what that means is the caller's business.
    pub(crate) fn static_prop_cell(
        &mut self,
        cid: u32,
        name: &[u8],
        scope: Option<u32>,
    ) -> Result<Rc<RefCell<Value>>, Unwind> {
        let class = self.classes[cid as usize].clone();
        let Some(&idx) = class.static_index.get(name) else {
            return Err(Unwind::error(format!(
                "Access to undeclared static property {}::${}",
                class.name_str(),
                String::from_utf8_lossy(name)
            )));
        };
        let info = &class.static_props[idx as usize];
        if !self.access_ok(info.vis, info.decl, scope) {
            return Err(Unwind::error(format!(
                "Cannot access {} property {}::${}",
                vis_word(info.vis),
                class.name_str(),
                String::from_utf8_lossy(name)
            )));
        }
        let cell = info.cell.clone();
        let ready = info.ready.clone();
        if ready.get() {
            return Ok(cell);
        }
        let init = info.init.clone();
        let (decl, ty) = (info.decl, info.ty.clone());
        // Mark ready *before* evaluating: an initializer that reaches its own
        // property must see the raw cell, not recurse.
        ready.set(true);
        let v = match init {
            None => return Ok(cell),
            Some(PropDefault::Value(v)) => v,
            Some(PropDefault::Thunk(fid)) => self.run_thunk(fid, None, Some(decl))?,
        };
        // A typed property with no initializer keeps `Uninit` (reading it is
        // an error until it is written). php rejects an explicit `= null` on
        // a non-nullable property at compile time, so a `null` there is the
        // "no initializer" spelling too, whichever one the compiler emits.
        let declared_null =
            matches!(v, Value::Null) && ty.as_ref().is_some_and(|t| !t.allows_null());
        if v.is_uninit() || declared_null {
            return Ok(cell);
        }
        let v = match &ty {
            Some(ty) => self.coerce_prop_value(decl, name, ty, v, true)?,
            None => v,
        };
        Value::assign(&mut cell.borrow_mut(), v);
        Ok(cell)
    }

    /// `Class::$name` read: the cell's value, with php's error for a typed
    /// property that was never initialized.
    pub(crate) fn fetch_static_prop(
        &mut self,
        cid: u32,
        name: &[u8],
        scope: Option<u32>,
    ) -> Result<Value, Unwind> {
        let cell = self.static_prop_cell(cid, name, scope)?;
        let v = cell.borrow().clone().unref();
        if v.is_uninit() {
            return Err(self.uninit_static_error(cid, name, false));
        }
        Ok(v)
    }

    /// `Class::$name = v`, through the shared cell (so a subclass that does
    /// not redeclare writes the parent's storage) and through a reference
    /// bound to it. Returns the stored value (`$x = A::$p = 1`).
    pub(crate) fn assign_static_prop(
        &mut self,
        cid: u32,
        name: &[u8],
        scope: Option<u32>,
        v: Value,
        strict: bool,
    ) -> Result<Value, Unwind> {
        let cell = self.static_prop_cell(cid, name, scope)?;
        let v = match self.static_prop_type(cid, name) {
            Some((decl, ty)) => self.coerce_prop_value(decl, name, &ty, v, strict)?,
            None => v,
        };
        Value::assign(&mut cell.borrow_mut(), v.clone());
        Ok(v)
    }

    /// `&Class::$name`: bind the shared cell to a reference, so a write
    /// through the reference is visible as the property.
    pub(crate) fn ref_static_prop(
        &mut self,
        cid: u32,
        name: &[u8],
        scope: Option<u32>,
    ) -> Result<PhpRef, Unwind> {
        let cell = self.static_prop_cell(cid, name, scope)?;
        let uninit = cell.borrow().is_uninit();
        if uninit {
            return Err(self.uninit_static_error(cid, name, true));
        }
        let mut slot = cell.borrow_mut();
        Ok(Value::make_ref(&mut slot))
    }

    /// `isset(Class::$name)`: never warns and never throws — an undeclared,
    /// invisible, uninitialized or null property is simply not set.
    pub(crate) fn isset_static_prop(&mut self, cid: u32, name: &[u8], scope: Option<u32>) -> bool {
        let class = self.classes[cid as usize].clone();
        let Some(&idx) = class.static_index.get(name) else {
            return false;
        };
        let info = &class.static_props[idx as usize];
        if !self.access_ok(info.vis, info.decl, scope) {
            return false;
        }
        match self.static_prop_cell(cid, name, scope) {
            Ok(cell) => {
                let v = cell.borrow().clone().unref();
                !matches!(v, Value::Null | Value::Uninit)
            }
            Err(_) => false,
        }
    }

    /// The declaring class and declared type of a static property, if it has
    /// one.
    fn static_prop_type(&self, cid: u32, name: &[u8]) -> Option<(u32, TypeDecl)> {
        let class = &self.classes[cid as usize];
        let &idx = class.static_index.get(name)?;
        let info = &class.static_props[idx as usize];
        info.ty.clone().map(|ty| (info.decl, ty))
    }

    /// php's error for reading a typed static property that has no value yet.
    fn uninit_static_error(&self, cid: u32, name: &[u8], by_ref: bool) -> Unwind {
        let class = self.classes[cid as usize].name_str();
        let name = String::from_utf8_lossy(name);
        if by_ref {
            Unwind::error(format!(
                "Cannot access uninitialized non-nullable property {class}::${name} by reference"
            ))
        } else {
            Unwind::error(format!(
                "Typed static property {class}::${name} must not be accessed before initialization"
            ))
        }
    }

    /// Coerce a value to a static property's declared type; php names the
    /// *declaring* class in the `TypeError`.
    fn coerce_prop_value(
        &mut self,
        decl: u32,
        name: &[u8],
        ty: &TypeDecl,
        v: Value,
        strict: bool,
    ) -> Result<Value, Unwind> {
        let given = self.given_name(&v);
        match self.coerce_to_type(v, ty, strict, Some(decl), Some(decl))? {
            Coerced::Ok(v) => Ok(v),
            Coerced::Mismatch => Err(Unwind::type_error(format!(
                "Cannot assign {given} to property {}::${} of type {}",
                self.classes[decl as usize].name_str(),
                String::from_utf8_lossy(name),
                self.type_display(ty, Some(decl))
            ))),
        }
    }

    /// The value of `Class::NAME`, evaluating the initializer on first use.
    ///
    /// This is the entry point for natives that resolve a class constant by
    /// name (`constant('A::X')`, `defined()`, reflection); the dispatch loop
    /// uses [`Interp::class_const_spelled`] so php's self-reference error can
    /// quote the source spelling.
    pub fn class_const(
        &mut self,
        cid: u32,
        name: &[u8],
        scope: Option<u32>,
    ) -> Result<Value, Unwind> {
        let spelling = self.classes[cid as usize].name_str();
        self.class_const_spelled(cid, name, scope, &spelling)
    }

    /// [`Interp::class_const`] with the spelling the source used for the
    /// class (`self`, `parent`, `static`, or a name): php's
    /// "Cannot declare self-referencing constant" quotes the *reference as
    /// written*, not the resolved class.
    pub fn class_const_spelled(
        &mut self,
        cid: u32,
        name: &[u8],
        scope: Option<u32>,
        spelling: &str,
    ) -> Result<Value, Unwind> {
        let class = self.classes[cid as usize].clone();
        let undefined = || {
            Unwind::error(format!(
                "Undefined constant {}::{}",
                class.name_str(),
                String::from_utf8_lossy(name)
            ))
        };
        let Some(k) = class.consts.get(name).cloned() else {
            return Err(undefined());
        };
        // A private constant is not inherited: through a subclass it does not
        // exist at all.
        if k.vis == Visibility::Private && k.decl != cid {
            return Err(undefined());
        }
        if !self.access_ok(k.vis, k.decl, scope) {
            return Err(Unwind::error(format!(
                "Cannot access {} constant {}::{}",
                vis_word(k.vis),
                class.name_str(),
                String::from_utf8_lossy(name)
            )));
        }
        self.eval_class_const(&k, name, spelling)
    }

    /// Evaluate a class constant's initializer once, in its declaring class's
    /// scope, with php's cycle detection.
    fn eval_class_const(
        &mut self,
        k: &Rc<ClassConst>,
        name: &[u8],
        spelling: &str,
    ) -> Result<Value, Unwind> {
        let pending = {
            let mut state = k.state.borrow_mut();
            match std::mem::replace(&mut *state, ConstState::Evaluating) {
                ConstState::Ready(v) => {
                    *state = ConstState::Ready(v.clone());
                    return Ok(v);
                }
                ConstState::Evaluating => {
                    return Err(Unwind::error(format!(
                        "Cannot declare self-referencing constant {spelling}::{}",
                        String::from_utf8_lossy(name)
                    )))
                }
                ConstState::Pending(p) => p,
            }
        };
        let evaluated = match &pending {
            PropDefault::Value(v) => Ok(v.clone()),
            PropDefault::Thunk(fid) => self.run_thunk(*fid, None, Some(k.decl)),
        };
        let checked = match (evaluated, &k.ty) {
            (Ok(v), Some(ty)) => {
                let ty = ty.clone();
                self.coerce_const_value(k, name, &ty, v)
            }
            (other, _) => other,
        };
        match checked {
            Ok(v) => {
                *k.state.borrow_mut() = ConstState::Ready(v.clone());
                Ok(v)
            }
            Err(u) => {
                // A failed evaluation leaves the constant unevaluated, so a
                // caught error does not turn it into `null`.
                *k.state.borrow_mut() = ConstState::Pending(pending);
                Err(u)
            }
        }
    }

    /// A typed class constant (php 8.3) checks its value when it evaluates.
    fn coerce_const_value(
        &mut self,
        k: &Rc<ClassConst>,
        name: &[u8],
        ty: &TypeDecl,
        v: Value,
    ) -> Result<Value, Unwind> {
        let given = self.given_name(&v);
        match self.coerce_to_type(v, ty, true, Some(k.decl), Some(k.decl))? {
            Coerced::Ok(v) => Ok(v),
            Coerced::Mismatch => Err(Unwind::type_error(format!(
                "Cannot assign {given} to class constant {}::{} of type {}",
                self.classes[k.decl as usize].name_str(),
                String::from_utf8_lossy(name),
                self.type_display(ty, Some(k.decl))
            ))),
        }
    }
}
