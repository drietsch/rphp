//! A closure value: an anonymous function plus its captured environment.
//!
//! `rphp-value` is below `rphp-bytecode`, so the function is stored as an opaque
//! `u32` id (the runtime interprets it as a `FuncId`); the captured variables are
//! plain [`Value`]s, snapshotted by value at the point the closure was created
//! (`function () use ($x)` / `fn () => $x`). Like other heap values it is
//! refcounted and cheaply cloned; identity is by pointer, matching PHP where two
//! distinct closures are never `==`.
use std::cell::RefCell;
use std::fmt;
use std::rc::{Rc, Weak};

use crate::{PhpRef, Value};

#[derive(Clone)]
pub struct Closure(Rc<ClosureData>);

struct ClosureData {
    func: u32,
    captures: Vec<Value>,
    /// The object handle (`spl_object_id()`, dumps): a closure is an object
    /// to php, so it takes one from the same allocator as every object.
    id: u32,
    /// The closure's own `static` variables. php gives **each closure
    /// object** a fresh set — two closures made from the same literal do not
    /// share a counter — so they cannot live on the compiled function the way
    /// a named function's statics do. `None` until the body first binds one.
    statics: RefCell<Option<Rc<RefCell<Vec<Option<PhpRef>>>>>>,
}

impl Closure {
    /// Create a closure over compiled function `func` capturing `captures`
    /// (in the order the function expects to bind them).
    pub fn new(func: u32, captures: Vec<Value>) -> Self {
        Closure::with_id(func, captures, 0)
    }

    /// [`Closure::new`] with the object handle `id`.
    pub fn with_id(func: u32, captures: Vec<Value>, id: u32) -> Self {
        Closure(Rc::new(ClosureData {
            func,
            captures,
            id,
            statics: RefCell::new(None),
        }))
    }

    /// The object handle (0 for a closure made without one).
    pub fn id(&self) -> u32 {
        self.0.id
    }

    /// A non-owning handle (`WeakMap` keys, `WeakReference`).
    pub fn downgrade(&self) -> WeakClosure {
        WeakClosure(Rc::downgrade(&self.0))
    }

    /// The compiled-function id this closure invokes.
    pub fn func(&self) -> u32 {
        self.0.func
    }

    /// The captured environment, in capture order.
    pub fn captures(&self) -> &[Value] {
        &self.0.captures
    }

    /// This closure's own static-variable table, created on first use with
    /// `len` slots. Every call of *this* closure shares it; a different
    /// closure object made from the same literal gets its own.
    pub fn statics(&self, len: usize) -> Rc<RefCell<Vec<Option<PhpRef>>>> {
        let mut slot = self.0.statics.borrow_mut();
        slot.get_or_insert_with(|| Rc::new(RefCell::new(vec![None; len])))
            .clone()
    }
}

impl PartialEq for Closure {
    /// Identity comparison: the same closure handle, never two distinct ones
    /// (PHP `==`/`===` on closures is reference identity).
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }
}

impl fmt::Debug for Closure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Closure(#{}, {} captures)", self.0.func, self.0.captures.len())
    }
}

/// A non-owning handle onto a closure (see [`Closure::downgrade`]).
#[derive(Clone)]
pub struct WeakClosure(Weak<ClosureData>);

impl WeakClosure {
    /// The closure, if it is still alive.
    pub fn upgrade(&self) -> Option<Closure> {
        self.0.upgrade().map(Closure)
    }

    /// Whether this weak handle points at `c`.
    pub fn ptr_eq_closure(&self, c: &Closure) -> bool {
        std::ptr::eq(self.0.as_ptr(), Rc::as_ptr(&c.0))
    }
}
