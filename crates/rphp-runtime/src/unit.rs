//! Loaded units and the runtime function / class tables (ADR-020).
//!
//! A [`Module`] is loaded into an [`Interp`] as a [`UnitRt`]: every function
//! of the module becomes a [`FuncRt`] with a process-wide id (the module's
//! unit-local [`FuncId`]s are offset by the unit's `func_base`), every class
//! a [`ClassRt`] (offset by `class_base`). Names are bound late: the
//! function table (`Interp::func_index`) and the class table
//! (`Interp::class_index`) map lowercased names to ids and are filled by
//! hoisting at load and by `DeclareFunction` / `DeclareClass` at run time;
//! each declaration bumps a generation counter that invalidates the inline
//! caches ([`IcSlot`]).

use std::cell::RefCell;
use std::rc::Rc;

use rphp_bytecode::{ClassId, FuncId, Function, Module, PropDef, Visibility};
use rphp_value::{Layout, PhpRef, PropMeta, Value};

use crate::registry::{NativeId, Unwind};
use crate::Interp;

/// One loaded module.
pub struct UnitRt {
    /// The file the unit was compiled from (`__FILE__`), or the unit name.
    pub file: Rc<str>,
    /// The process-wide id of the unit's `funcs[0]`.
    pub func_base: u32,
    /// The process-wide id of the unit's `classes[0]`.
    pub class_base: u32,
    /// The unit's `{main}` (process-wide id).
    pub main: u32,
}

impl UnitRt {
    /// The process-wide id of a unit-local function id.
    pub fn func_id(&self, local: FuncId) -> u32 {
        self.func_base + local
    }

    /// The process-wide id of a unit-local class id.
    pub fn class_id(&self, local: ClassId) -> u32 {
        self.class_base + local
    }
}

/// An inline-cache slot, stamped with the generation of the table it was
/// resolved against.
#[derive(Clone)]
pub enum IcSlot {
    /// Nothing cached yet.
    Empty,
    /// A resolved user function.
    Func { gen: u32, id: u32 },
    /// A resolved native.
    Native { gen: u32, id: NativeId },
    /// A resolved class.
    Class { gen: u32, id: u32 },
}

/// A compiled function as the runtime holds it.
pub struct FuncRt {
    /// Process-wide id.
    pub id: u32,
    /// The compiled body and metadata.
    pub f: Function,
    /// The owning unit.
    pub unit: Rc<UnitRt>,
    /// Inline caches (`Function::ic_count` slots).
    pub ics: RefCell<Vec<IcSlot>>,
    /// `static $x` cells, created on first `BindStatic`.
    pub statics: RefCell<Vec<Option<PhpRef>>>,
    /// The declaring class (process-wide id) for methods.
    pub class: Option<u32>,
    /// Register → variable name (the inverse of `Function::var_names`), for
    /// symbol-table rebinding.
    pub reg_names: Vec<Option<Box<[u8]>>>,
}

impl FuncRt {
    /// Whether this is the unit's `{main}`.
    pub fn is_main(&self) -> bool {
        self.id == self.unit.main
    }

    /// The variable name of register `reg`, if it is a named variable.
    pub fn reg_name(&self, reg: u16) -> Option<&[u8]> {
        self.reg_names
            .get(reg as usize)
            .and_then(|n| n.as_deref())
    }

    /// The source line of the op at `pc` (0 without line information).
    pub fn line_at(&self, pc: usize) -> u32 {
        self.f.line_at(pc).unwrap_or(0)
    }
}

/// A declared class as the runtime holds it (the M0 `Class` shape, plan E6
/// replaces it with the `ClassDef` built from `ClassDecl`).
pub struct ClassRt {
    /// Process-wide id.
    pub id: u32,
    /// Declared name (original case).
    pub name: Box<[u8]>,
    /// Parent class (process-wide id).
    pub parent: Option<u32>,
    /// Own declared properties.
    pub props: Vec<PropDef>,
    /// Own methods: (name, process-wide function id, visibility).
    pub methods: Vec<(Box<[u8]>, u32, Visibility)>,
    /// The instance layout (parent-first property set) and default slots.
    pub layout: Rc<Layout>,
    /// Default slot values, parallel to the layout.
    pub defaults: Vec<Value>,
    /// The unit that declared it and the declaration line (for
    /// "previously declared in" messages); `None` for engine classes.
    pub declared_at: Option<(Rc<str>, u32)>,
}

impl Interp {
    /// The function with process-wide id `id`.
    pub fn func(&self, id: u32) -> &Rc<FuncRt> {
        &self.funcs[id as usize]
    }

    /// The class with process-wide id `id`.
    pub fn class(&self, id: u32) -> &Rc<ClassRt> {
        &self.classes[id as usize]
    }

    /// Look a (case-insensitive) function name up in the user function table.
    pub fn user_function(&self, name: &[u8]) -> Option<u32> {
        let key = name.to_ascii_lowercase();
        self.func_index.get(key.as_slice()).copied()
    }

    /// Look a (case-insensitive) class name up (no autoload yet).
    pub fn class_by_name(&self, name: &[u8]) -> Option<u32> {
        let name = name.strip_prefix(b"\\").unwrap_or(name);
        let key = name.to_ascii_lowercase();
        self.class_index.get(key.as_slice()).copied()
    }

    /// Whether `class` is `ancestor` or descends from it.
    pub fn is_subclass_or_eq(&self, class: u32, ancestor: u32) -> bool {
        let mut cur = Some(class);
        while let Some(cid) = cur {
            if cid == ancestor {
                return true;
            }
            cur = self.classes[cid as usize].parent;
        }
        false
    }

    /// Resolve a method by (case-insensitive) name on `class`, walking up the
    /// chain: the function id, its visibility and the declaring class.
    pub fn resolve_method(&self, class: u32, name: &[u8]) -> Option<(u32, Visibility, u32)> {
        let mut cur = Some(class);
        while let Some(cid) = cur {
            let c = &self.classes[cid as usize];
            if let Some((_, fid, vis)) = c.methods.iter().find(|(n, _, _)| n.eq_ignore_ascii_case(name)) {
                return Some((*fid, *vis, cid));
            }
            cur = c.parent;
        }
        None
    }

    /// Resolve a declared property's visibility and declaring class, walking
    /// up the chain. `None` for an undeclared (dynamic) property.
    pub fn resolve_prop(&self, class: u32, name: &[u8]) -> Option<(Visibility, u32)> {
        let mut cur = Some(class);
        while let Some(cid) = cur {
            let c = &self.classes[cid as usize];
            if let Some(p) = c.props.iter().find(|p| p.name.as_ref() == name) {
                return Some((p.visibility, cid));
            }
            cur = c.parent;
        }
        None
    }

    /// Install a compiled module: every function and class gets a
    /// process-wide id, hoisted functions and classes are declared, and the
    /// unit's `{main}` id is returned. Nothing runs.
    pub fn load_unit(&mut self, module: Module) -> Result<u32, Unwind> {
        let func_base = self.funcs.len() as u32;
        let class_base = self.classes.len() as u32;
        let unit = Rc::new(UnitRt {
            file: module.file.clone(),
            func_base,
            class_base,
            main: func_base + module.main,
        });
        let Module {
            funcs,
            classes,
            hoist_funcs,
            hoist_classes,
            ..
        } = module;
        for (i, f) in funcs.into_iter().enumerate() {
            let mut reg_names: Vec<Option<Box<[u8]>>> = vec![None; f.num_regs as usize];
            for (name, reg) in &f.var_names {
                if let Some(slot) = reg_names.get_mut(*reg as usize) {
                    *slot = Some(name.clone());
                }
            }
            let class = f.in_class.map(|c| class_base + c);
            let ics = vec![IcSlot::Empty; f.ic_count as usize];
            let statics = vec![None; f.statics.len()];
            self.funcs.push(Rc::new(FuncRt {
                id: func_base + i as u32,
                f,
                unit: unit.clone(),
                ics: RefCell::new(ics),
                statics: RefCell::new(statics),
                class,
                reg_names,
            }));
        }
        for (i, c) in classes.into_iter().enumerate() {
            let id = class_base + i as u32;
            let parent = c.parent.map(|p| class_base + p);
            let methods = c
                .methods
                .iter()
                .map(|m| (m.name_bytes.clone(), func_base + m.func, m.visibility))
                .collect();
            // Placeholder layout; finalized when the class is declared (its
            // parent's layout must exist by then).
            let rt = ClassRt {
                id,
                name: c.name_bytes.clone(),
                parent,
                props: c.props.clone(),
                methods,
                layout: Rc::new(Layout::empty(Rc::from(&c.name_bytes[..]))),
                defaults: Vec::new(),
                declared_at: Some((unit.file.clone(), c.line)),
            };
            self.classes.push(Rc::new(rt));
        }
        self.units.push(unit.clone());
        for fid in hoist_funcs {
            self.declare_function(func_base + fid)?;
        }
        for cid in hoist_classes {
            self.declare_class(class_base + cid)?;
        }
        Ok(unit.main)
    }

    /// Declare function `id` in the function table (`DeclareFunction` and
    /// hoisting). Nameless functions (closures, thunks) are skipped.
    pub fn declare_function(&mut self, id: u32) -> Result<(), Unwind> {
        let f = self.funcs[id as usize].clone();
        if f.f.name_bytes.is_empty() {
            return Ok(());
        }
        let key: Box<[u8]> = f.f.name_bytes.to_ascii_lowercase().into();
        if self.native_index.contains_key(&key) {
            // php: redeclaring an internal function is a compile-time fatal
            // without a "previously declared in" location.
            let msg = format!(
                "Cannot redeclare function {}()",
                String::from_utf8_lossy(&f.f.name_bytes)
            );
            let file = f.unit.file.to_string();
            return Err(self.fatal_at(&msg, &file, f.f.decl_line));
        }
        if let Some(&prev) = self.func_index.get(&key) {
            let p = self.funcs[prev as usize].clone();
            let msg = format!(
                "Cannot redeclare function {}() (previously declared in {}:{})",
                String::from_utf8_lossy(&f.f.name_bytes),
                p.unit.file,
                p.f.decl_line
            );
            let file = f.unit.file.to_string();
            return Err(self.fatal_at(&msg, &file, f.f.decl_line));
        }
        self.func_index.insert(key, id);
        self.func_gen += 1;
        Ok(())
    }

    /// Declare class `id` in the class table (`DeclareClass` and hoisting):
    /// resolves its parent's layout and builds its own.
    pub fn declare_class(&mut self, id: u32) -> Result<(), Unwind> {
        let c = self.classes[id as usize].clone();
        let key: Box<[u8]> = c.name.to_ascii_lowercase().into();
        if let Some(&prev) = self.class_index.get(&key) {
            let p = self.classes[prev as usize].clone();
            let (file, line) = p
                .declared_at
                .clone()
                .unwrap_or((Rc::from("Unknown"), 0));
            let msg = format!(
                "Cannot redeclare class {} (previously declared in {}:{})",
                String::from_utf8_lossy(&p.name),
                file,
                line
            );
            let (here_file, here_line) = c
                .declared_at
                .clone()
                .unwrap_or((Rc::from("Unknown"), 0));
            return Err(self.fatal_at(&msg, &here_file, here_line));
        }
        // Parent-first property set; a redeclaration overrides the default.
        let mut chain = Vec::new();
        let mut cur = Some(id);
        let mut hops = 0;
        while let Some(cid) = cur {
            chain.push(cid);
            cur = self.classes[cid as usize].parent;
            hops += 1;
            if hops > self.classes.len() {
                return Err(self.fatal(&format!(
                    "Class {} has a cyclic inheritance chain",
                    String::from_utf8_lossy(&c.name)
                )));
            }
        }
        let mut metas: Vec<PropMeta> = Vec::new();
        let mut defaults: Vec<Value> = Vec::new();
        for &cid in chain.iter().rev() {
            let cls = self.classes[cid as usize].clone();
            for p in &cls.props {
                let vis = match p.visibility {
                    Visibility::Public => rphp_value::Vis::Public,
                    Visibility::Protected => rphp_value::Vis::Protected,
                    Visibility::Private => rphp_value::Vis::Private,
                };
                if let Some(i) = metas.iter().position(|m| m.name.as_ref() == p.name.as_ref()) {
                    metas[i].vis = vis;
                    metas[i].decl_class = cid;
                    metas[i].decl_class_name = Rc::from(&cls.name[..]);
                    defaults[i] = p.default.clone();
                } else {
                    metas.push(PropMeta {
                        name: p.name.clone(),
                        vis,
                        decl_class: cid,
                        decl_class_name: Rc::from(&cls.name[..]),
                    });
                    defaults.push(p.default.clone());
                }
            }
        }
        let layout = Rc::new(Layout::new(Rc::from(&c.name[..]), metas));
        let rt = ClassRt {
            id,
            name: c.name.clone(),
            parent: c.parent,
            props: c.props.clone(),
            methods: c.methods.clone(),
            layout,
            defaults,
            declared_at: c.declared_at.clone(),
        };
        self.classes[id as usize] = Rc::new(rt);
        self.class_index.insert(key, id);
        self.class_gen += 1;
        Ok(())
    }

    /// Register an engine-provided class with no members (`stdClass`).
    pub(crate) fn register_builtin_class(&mut self, name: &[u8]) -> u32 {
        let id = self.classes.len() as u32;
        let rt = ClassRt {
            id,
            name: Box::from(name),
            parent: None,
            props: Vec::new(),
            methods: Vec::new(),
            layout: Rc::new(Layout::empty(Rc::from(name))),
            defaults: Vec::new(),
            declared_at: None,
        };
        self.classes.push(Rc::new(rt));
        self.class_index
            .insert(name.to_ascii_lowercase().into_boxed_slice(), id);
        self.class_gen += 1;
        id
    }

    /// `new <class>` without running a constructor: an instance with its
    /// default slots and the next object id.
    pub fn instantiate(&mut self, class: u32) -> rphp_value::Object {
        let c = self.classes[class as usize].clone();
        let id = self.object_ids.alloc();
        rphp_value::Object::new(class, id, c.layout.clone(), c.defaults.clone())
    }

    /// The class name of an object (its runtime class).
    pub fn class_name_of(&self, o: &rphp_value::Object) -> String {
        String::from_utf8_lossy(&self.classes[o.class_id() as usize].name).into_owned()
    }
}
