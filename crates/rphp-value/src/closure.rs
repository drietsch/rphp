//! A closure value: an anonymous function plus its captured environment.
//!
//! `rphp-value` is below `rphp-bytecode`, so the function is stored as an opaque
//! `u32` id (the runtime interprets it as a `FuncId`); the captured variables are
//! plain [`Value`]s, snapshotted by value at the point the closure was created
//! (`function () use ($x)` / `fn () => $x`). Like other heap values it is
//! refcounted and cheaply cloned; identity is by pointer, matching PHP where two
//! distinct closures are never `==`.
use std::fmt;
use std::rc::Rc;

use crate::Value;

#[derive(Clone)]
pub struct Closure(Rc<ClosureData>);

struct ClosureData {
    func: u32,
    captures: Vec<Value>,
}

impl Closure {
    /// Create a closure over compiled function `func` capturing `captures`
    /// (in the order the function expects to bind them).
    pub fn new(func: u32, captures: Vec<Value>) -> Self {
        Closure(Rc::new(ClosureData { func, captures }))
    }

    /// The compiled-function id this closure invokes.
    pub fn func(&self) -> u32 {
        self.0.func
    }

    /// The captured environment, in capture order.
    pub fn captures(&self) -> &[Value] {
        &self.0.captures
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
