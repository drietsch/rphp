//! Magic-constant substitution.
//!
//! Every rule was established against php 8.5.0 (`tests/magic.rs` records
//! the scripts):
//!
//! | constant        | value                                                                 |
//! |-----------------|-----------------------------------------------------------------------|
//! | `__LINE__`      | the token's line                                                       |
//! | `__FILE__`      | the absolute script path; `<file>(<line>) : eval()'d code` in `eval`   |
//! | `__DIR__`       | `dirname()` of the *script* (also inside `eval`)                       |
//! | `__NAMESPACE__` | the current namespace (`""` at global scope)                           |
//! | `__FUNCTION__`  | `""` at top level (also at the top level of eval'd code), `Ns\f` for a function (also in the constants of a class declared inside it), `m` for a method, `$p::get` in a hook, `{closure:<owner>:<line>}` for closures where `<owner>` is the enclosing function's `__METHOD__` value with `()` appended (`Ns\f()`, `Ns\C::m()`, `Ns\C::$p::get()`, an enclosing closure's own name unchanged) or `__FILE__` at top level |
//! | `__METHOD__`    | like `__FUNCTION__` but `Ns\C::m` for methods, `Ns\C::$p::get` for hooks, and `""` directly inside a class body (constants, defaults) unless a closure encloses the class |
//! | `__CLASS__`     | the class FQN inside a class (also in closures inside methods and in class constants / property defaults); `""` outside and inside free functions; **kept dynamic** inside traits (php resolves it to the using class at run time) and inside anonymous classes (the generated name carries a compiler counter) |
//! | `__TRAIT__`     | the trait FQN inside a trait, `""` elsewhere                           |
//! | `__PROPERTY__`  | the property name directly inside a hook body, `""` elsewhere (a closure inside a hook sees `""`) |
//!
//! `__CLASS__`/`__METHOD__` inside an anonymous class and `__FUNCTION__`/
//! `__METHOD__` of a closure lexically inside one stay `MagicConst` for the
//! compiler, which owns the anonymous-class name.

use rphp_ast::v2::MagicKind;

use crate::scope::{ClassScope, Entry, FnKind, ScopeStack};

/// The scope facts magic substitution needs, borrowed from the resolver.
pub struct MagicCtx<'a> {
    /// `__FILE__`.
    pub file: &'a [u8],
    /// `__DIR__`.
    pub dir: &'a [u8],
    /// The current namespace name (empty at global scope).
    pub namespace: &'a [u8],
    /// The lexical stack.
    pub stack: &'a ScopeStack,
}

/// The outcome of substituting one magic constant.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Magic {
    /// Replace with this string.
    Str(Vec<u8>),
    /// Keep the `MagicConst` node: the value is only known at run time.
    Dynamic,
}

impl MagicCtx<'_> {
    /// The class in effect for the function-like at stack index `idx`.
    fn class_for_fn(&self, idx: usize) -> Option<&ClassScope> {
        self.stack.effective_class_from(idx)
    }

    /// `__METHOD__` of the function-like at `idx`; `None` when it would
    /// embed an anonymous class name.
    fn method_name_at(&self, idx: usize) -> Option<Vec<u8>> {
        let Entry::Fn(scope) = &self.stack.entries[idx] else {
            return Some(Vec::new());
        };
        match &scope.kind {
            FnKind::Main => Some(Vec::new()),
            FnKind::Function { fqn } => Some(fqn.clone()),
            FnKind::Method { name } => {
                let mut out = self.class_prefix(idx)?;
                out.extend_from_slice(name);
                Some(out)
            }
            FnKind::Hook { prop, kind } => {
                let mut out = self.class_prefix(idx)?;
                out.push(b'$');
                out.extend_from_slice(prop);
                out.extend_from_slice(b"::");
                out.extend_from_slice(kind.as_str().as_bytes());
                Some(out)
            }
            FnKind::Closure { line } => {
                let mut out = b"{closure:".to_vec();
                let owner = match self.stack.fn_below(idx) {
                    None => Vec::new(),
                    Some((oi, of)) => {
                        let mut o = self.method_name_at(oi)?;
                        if !o.is_empty() && !matches!(of.kind, FnKind::Closure { .. }) {
                            o.extend_from_slice(b"()");
                        }
                        o
                    }
                };
                if owner.is_empty() {
                    out.extend_from_slice(self.file);
                } else {
                    out.extend_from_slice(&owner);
                }
                out.push(b':');
                out.extend_from_slice(line.to_string().as_bytes());
                out.push(b'}');
                Some(out)
            }
        }
    }

    /// `Ns\C::` for the class enclosing the function-like at `idx`; empty
    /// without a class; `None` for an anonymous class.
    fn class_prefix(&self, idx: usize) -> Option<Vec<u8>> {
        match self.class_for_fn(idx) {
            None => Some(Vec::new()),
            Some(c) => {
                let mut out = c.fqn.clone()?;
                out.extend_from_slice(b"::");
                Some(out)
            }
        }
    }

    /// The substituted value of `kind`. `__LINE__` is the caller's job (it
    /// needs the span and the line table).
    pub fn value(&self, kind: MagicKind) -> Magic {
        let (fi, f) = self.stack.innermost_fn();
        let dyn_or = |v: Option<Vec<u8>>| v.map_or(Magic::Dynamic, Magic::Str);
        match kind {
            MagicKind::Line => Magic::Dynamic,
            MagicKind::File => Magic::Str(self.file.to_vec()),
            MagicKind::Dir => Magic::Str(self.dir.to_vec()),
            MagicKind::Namespace => Magic::Str(self.namespace.to_vec()),
            MagicKind::Class => match self.stack.effective_class() {
                None => Magic::Str(Vec::new()),
                Some(c) if c.is_trait() => Magic::Dynamic,
                Some(c) => dyn_or(c.fqn.clone()),
            },
            MagicKind::Trait => Magic::Str(
                self.stack
                    .effective_class()
                    .filter(|c| c.is_trait())
                    .and_then(|c| c.fqn.clone())
                    .unwrap_or_default(),
            ),
            MagicKind::Property => {
                Magic::Str(match (&f.kind, self.stack.directly_in_class_body()) {
                    (FnKind::Hook { prop, .. }, false) => prop.clone(),
                    _ => Vec::new(),
                })
            }
            MagicKind::Function => match &f.kind {
                FnKind::Main => Magic::Str(Vec::new()),
                FnKind::Function { fqn } => Magic::Str(fqn.clone()),
                FnKind::Method { name } => Magic::Str(name.clone()),
                FnKind::Hook { prop, kind } => {
                    let mut out = vec![b'$'];
                    out.extend_from_slice(prop);
                    out.extend_from_slice(b"::");
                    out.extend_from_slice(kind.as_str().as_bytes());
                    Magic::Str(out)
                }
                FnKind::Closure { .. } => dyn_or(self.method_name_at(fi)),
            },
            MagicKind::Method => {
                // Directly inside a class body php treats the position as
                // "not inside a function" — unless the enclosing op_array is
                // a closure.
                if self.stack.directly_in_class_body() && !matches!(f.kind, FnKind::Closure { .. })
                {
                    return Magic::Str(Vec::new());
                }
                dyn_or(self.method_name_at(fi))
            }
        }
    }
}
