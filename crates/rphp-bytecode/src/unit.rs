//! Compilation units: the M0 [`Module`] (a whole program linked at compile
//! time) and the v2 [`CompiledUnit`] (one file or `eval` string, declared
//! incrementally into the interpreter, plan E7).

use std::rc::Rc;
use std::sync::Arc;

use rphp_value::{Value, Vis};

use crate::{vis_to_value, Class, ClassDecl, ClassId, FuncId, Function, Visibility};

/// M0: the whole program, with functions and classes resolved to ids at
/// compile time. (Superseded by [`CompiledUnit`]; kept for the current
/// compiler/runtime pair.)
#[derive(Clone, Debug)]
pub struct Module {
    /// Every compiled function, shared: an interpreter links a unit by
    /// taking handles on these (a server's worker links the same cached
    /// unit for every request).
    pub funcs: Vec<Rc<Function>>,
    /// Declared classes, indexed by [`ClassId`].
    pub classes: Vec<Class>,
    /// The synthetic top-level `{main}` function id.
    pub main: FuncId,
    /// Unconditional top-level function declarations, declared into the
    /// interpreter's function table before `{main}` runs (E3, CONTRACT.md
    /// §8). Every other named function in `funcs` (a conditional / nested
    /// declaration) is declared by its [`Op::DeclareFunction`](crate::Op::DeclareFunction).
    pub hoist_funcs: Vec<FuncId>,
    /// Top-level class declarations, declared before `{main}` runs; the rest
    /// by their [`Op::DeclareClass`](crate::Op::DeclareClass).
    pub hoist_classes: Vec<ClassId>,
    /// The file the module was compiled from (`__FILE__`), or the unit name
    /// (`Command line code`) for `-r` / eval.
    pub file: Arc<str>,
}

impl Module {
    /// A module whose `funcs[main]` is the entry function, with every function
    /// and class hoisted (the M0 shape: nothing conditional).
    pub fn new_hoisted(funcs: Vec<Function>, classes: Vec<Class>, main: FuncId) -> Module {
        let hoist_funcs = (0..funcs.len() as FuncId).filter(|&i| i != main).collect();
        let hoist_classes = (0..classes.len() as ClassId).collect();
        Module {
            funcs: funcs.into_iter().map(Rc::new).collect(),
            classes,
            main,
            hoist_funcs,
            hoist_classes,
            file: Arc::from("Command line code"),
        }
    }
}

impl Module {
    pub fn func(&self, id: FuncId) -> &Function {
        &self.funcs[id as usize]
    }

    pub fn class(&self, id: ClassId) -> &Class {
        &self.classes[id as usize]
    }

    /// Resolve a function name (case-insensitive, as PHP) to its id. Used to turn
    /// a callable string into a callable target at runtime.
    pub fn func_by_name(&self, name: &[u8]) -> Option<FuncId> {
        self.funcs
            .iter()
            .position(|f| f.name_bytes.eq_ignore_ascii_case(name))
            .map(|i| i as FuncId)
    }

    /// Resolve a class name (case-insensitive, as PHP) to its id.
    pub fn class_by_name(&self, name: &[u8]) -> Option<ClassId> {
        self.classes
            .iter()
            .position(|c| c.name_bytes.eq_ignore_ascii_case(name))
            .map(|i| i as ClassId)
    }

    /// Resolve a method by name on `class`, walking up the inheritance chain.
    /// Returns the compiled function, its visibility, and the class in the chain
    /// that *declares* it (the lexical context for visibility checks).
    pub fn resolve_method(
        &self,
        class: ClassId,
        name: &[u8],
    ) -> Option<(FuncId, Visibility, ClassId)> {
        let mut cur = Some(class);
        while let Some(cid) = cur {
            let c = self.class(cid);
            if let Some(m) = c
                .methods
                .iter()
                .find(|m| m.name_bytes.eq_ignore_ascii_case(name))
            {
                return Some((m.func, m.visibility, cid));
            }
            cur = c.parent;
        }
        None
    }

    /// Resolve a declared property's visibility and declaring class, walking up
    /// the chain. `None` for an undeclared (dynamic) property — those are public.
    pub fn resolve_prop(&self, class: ClassId, name: &[u8]) -> Option<(Visibility, ClassId)> {
        let mut cur = Some(class);
        while let Some(cid) = cur {
            let c = self.class(cid);
            if let Some(p) = c.props.iter().find(|p| p.name.as_ref() == name) {
                return Some((p.visibility, cid));
            }
            cur = c.parent;
        }
        None
    }

    /// The [`ClassId`] that declares the method compiled to `func`, if any.
    pub fn method_owner(&self, func: FuncId) -> Option<ClassId> {
        self.classes
            .iter()
            .position(|c| c.methods.iter().any(|m| m.func == func))
            .map(|i| i as ClassId)
    }

    /// An instance's full property set (name, default, visibility), parent-first
    /// with a subclass redeclaration overriding the inherited default — the
    /// layout a fresh `new <class>` is seeded with.
    pub fn instance_props(&self, class: ClassId) -> Vec<(Box<[u8]>, Value, Vis)> {
        // Collect the chain root-first so children override parents by name.
        let mut chain = Vec::new();
        let mut cur = Some(class);
        while let Some(cid) = cur {
            chain.push(cid);
            cur = self.class(cid).parent;
        }
        let mut out: Vec<(Box<[u8]>, Value, Vis)> = Vec::new();
        for &cid in chain.iter().rev() {
            for p in &self.class(cid).props {
                let vis = vis_to_value(p.visibility);
                if let Some(slot) = out
                    .iter_mut()
                    .find(|(n, _, _)| n.as_ref() == p.name.as_ref())
                {
                    slot.1 = p.default.clone();
                    slot.2 = vis;
                } else {
                    out.push((p.name.clone(), p.default.clone(), vis));
                }
            }
        }
        out
    }

    /// Whether `class` is `ancestor` or descends from it.
    pub fn is_subclass_or_eq(&self, class: ClassId, ancestor: ClassId) -> bool {
        let mut cur = Some(class);
        while let Some(cid) = cur {
            if cid == ancestor {
                return true;
            }
            cur = self.class(cid).parent;
        }
        false
    }
}

/// v2: one compiled source unit — a file, or an `eval()` string — as handed to
/// `Interp::load_unit` (plan E7). Nothing in it is resolved across units:
/// functions and classes are declared into the interpreter's tables by name,
/// hoisted ones at load and the rest by [`Op::DeclareFunction`] /
/// [`Op::DeclareClass`] in statement order, and every cross-unit reference in
/// the code is late-bound through [`Const::Name`].
///
/// [`Op::DeclareFunction`]: crate::Op::DeclareFunction
/// [`Op::DeclareClass`]: crate::Op::DeclareClass
/// [`Const::Name`]: crate::Const::Name
#[derive(Clone, Debug)]
pub struct CompiledUnit {
    /// The file path as PHP reports it (`__FILE__`; for eval,
    /// `<file>(<line>) : eval()'d code`).
    pub file: Arc<str>,
    /// Every function in the unit: `{main}`, named functions, methods, closures,
    /// hooks and thunks. [`FuncId`]s are indices into this vector.
    pub funcs: Vec<Function>,
    /// Every class-like declaration; [`ClassId`]s index it.
    pub classes: Vec<ClassDecl>,
    /// The `{main}` function (`FnFlags::NEEDS_SYMTAB`), run as an `Include` frame.
    pub main: FuncId,
    /// Unconditional top-level function declarations, declared before `{main}`
    /// runs (so a call can precede the declaration in the file).
    pub hoist_funcs: Vec<FuncId>,
    /// Early-bindable top-level classes (no parent/interfaces/traits, or a
    /// parent already loaded at compile time), declared before `{main}` runs.
    /// All other classes are declared by their `DeclareClass` op.
    pub hoist_classes: Vec<ClassId>,
    /// `declare(strict_types=1)` at the top of the file (mirrored into every
    /// function's `FnFlags::STRICT_TYPES`).
    pub strict_types: bool,
    /// Byte offset just past `__halt_compiler();` — the value of
    /// `__COMPILER_HALT_OFFSET__` — when the file has one.
    pub halt_offset: Option<u32>,
}

impl CompiledUnit {
    /// The function with this id.
    pub fn func(&self, id: FuncId) -> &Function {
        &self.funcs[id as usize]
    }

    /// The class declaration with this id.
    pub fn class(&self, id: ClassId) -> &ClassDecl {
        &self.classes[id as usize]
    }

    /// An empty unit for `file` whose `{main}` is `funcs[0]` (an empty
    /// function), for struct-update construction.
    pub fn new_empty(file: &str) -> CompiledUnit {
        CompiledUnit {
            file: Arc::from(file),
            funcs: vec![Function::default()],
            classes: Vec::new(),
            main: 0,
            hoist_funcs: Vec::new(),
            hoist_classes: Vec::new(),
            strict_types: false,
            halt_offset: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_unit_has_a_main() {
        let u = CompiledUnit::new_empty("/tmp/x.php");
        assert_eq!(&*u.file, "/tmp/x.php");
        assert!(u.func(u.main).code.is_empty());
        assert!(u.classes.is_empty() && u.hoist_funcs.is_empty() && !u.strict_types);
    }
}
