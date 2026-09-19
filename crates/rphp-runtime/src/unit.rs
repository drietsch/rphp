//! Loaded units and the runtime function / class tables (ADR-020).
//!
//! A [`Module`] is loaded into an [`Interp`] as a [`UnitRt`]: every function
//! of the module becomes a [`FuncRt`] with a process-wide id (the module's
//! unit-local [`FuncId`]s are offset by the unit's `func_base`), every class
//! a [`ClassDef`] (offset by `class_base`). Names are bound late: the
//! function table (`Interp::func_index`) and the class table
//! (`Interp::class_index`) map lowercased names to ids and are filled by
//! hoisting at load and by `DeclareFunction` / `DeclareClass` at run time;
//! each declaration bumps a generation counter that invalidates the inline
//! caches ([`IcSlot`]).

use std::cell::RefCell;
use std::rc::Rc;

use rphp_bytecode::{ClassId, FuncId, Function, Module, Visibility};
use rphp_value::{PhpRef, Value};

use crate::class::{ClassDef, ClassSpec, MagicFlags, MethodBody, MethodDef, MethodSpec, PropDefault};
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
    /// The unit's compiled class declarations (unit-local ids), linked by
    /// `declare_class`.
    pub decls: Vec<rphp_bytecode::Class>,
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

impl Interp {
    /// The function with process-wide id `id`.
    pub fn func(&self, id: u32) -> &Rc<FuncRt> {
        &self.funcs[id as usize]
    }

    /// The class with process-wide id `id`.
    pub fn class(&self, id: u32) -> &Rc<ClassDef> {
        &self.classes[id as usize]
    }

    /// Every class in the table (declared or not), by id.
    pub fn classes(&self) -> &[Rc<ClassDef>] {
        &self.classes
    }

    /// The declared classes in declaration order (`get_declared_classes()`).
    pub fn declared_classes(&self) -> impl Iterator<Item = &Rc<ClassDef>> {
        self.class_order.iter().map(|&id| &self.classes[id as usize])
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

    /// Whether `class` is `ancestor`, descends from it or implements it
    /// (kept under its E3 name for callers; see
    /// [`Interp::instanceof_class`]).
    pub fn is_subclass_or_eq(&self, class: u32, ancestor: u32) -> bool {
        self.instanceof_class(class, ancestor)
    }

    /// Resolve a method by (case-insensitive) name on `class` (own or
    /// inherited).
    pub fn resolve_method(&self, class: u32, name: &[u8]) -> Option<Rc<MethodDef>> {
        self.classes[class as usize].method(name).cloned()
    }

    /// Resolve a declared property's visibility and declaring class.
    /// `None` for an undeclared (dynamic) property.
    pub fn resolve_prop(&self, class: u32, name: &[u8]) -> Option<(Visibility, u32)> {
        self.classes[class as usize].prop(name).map(|p| (p.vis, p.decl))
    }

    /// Install a compiled module: every function and class gets a
    /// process-wide id, hoisted functions and classes are declared, and the
    /// unit's `{main}` id is returned. Nothing runs.
    pub fn load_unit(&mut self, module: Module) -> Result<u32, Unwind> {
        let func_base = self.funcs.len() as u32;
        let class_base = self.classes.len() as u32;
        let Module {
            funcs,
            classes,
            hoist_funcs,
            hoist_classes,
            main,
            file,
        } = module;
        let unit = Rc::new(UnitRt {
            file,
            func_base,
            class_base,
            main: func_base + main,
            decls: classes,
        });
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
        for (i, c) in unit.decls.iter().enumerate() {
            let id = class_base + i as u32;
            let mut stub = ClassDef::stub(
                id,
                &c.name_bytes,
                c.kind,
                c.flags,
                Some((unit.file.clone(), c.line)),
            );
            stub.unit = Some(unit.clone());
            self.classes.push(Rc::new(stub));
        }
        self.units.push(unit.clone());
        for fid in hoist_funcs {
            self.declare_function(func_base + fid)?;
        }
        // Parents first: a hoisted class whose parent is another hoisted
        // class of this unit waits for it.
        let mut pending: Vec<u32> = hoist_classes.iter().map(|&c| class_base + c).collect();
        let mut progress = true;
        while !pending.is_empty() && progress {
            progress = false;
            let mut rest = Vec::new();
            for cid in pending {
                let decl = &unit.decls[(cid - class_base) as usize];
                let parent_ready = match self.parent_id_of(decl, class_base) {
                    Some(pid) => self.classes[pid as usize].linked,
                    None => true,
                };
                if parent_ready {
                    self.declare_class(cid)?;
                    progress = true;
                } else {
                    rest.push(cid);
                }
            }
            pending = rest;
        }
        for cid in pending {
            self.declare_class(cid)?;
        }
        Ok(unit.main)
    }

    /// The process-wide id of a compiled class's parent: by name in the
    /// class table, else the unit-local hint.
    fn parent_id_of(&self, decl: &rphp_bytecode::Class, class_base: u32) -> Option<u32> {
        if let Some(name) = &decl.parent_name {
            if let Some(id) = self.class_by_name(name) {
                return Some(id);
            }
        }
        decl.parent.map(|p| class_base + p)
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
    /// resolve its parent and interfaces, link it ([`Interp::link_class`])
    /// and publish it.
    pub fn declare_class(&mut self, id: u32) -> Result<(), Unwind> {
        let stub = self.classes[id as usize].clone();
        let Some(unit) = stub.unit.clone() else {
            return Ok(());
        };
        let decl = &unit.decls[(id - unit.class_base) as usize];
        let (here_file, here_line) = stub
            .declared_at
            .clone()
            .unwrap_or((Rc::from("Unknown"), 0));
        // php gives every `new class` site one class entry, whatever happens
        // to run it again — a loop, a function called twice — so declaring it
        // a second time is a no-op rather than a redeclaration.
        if stub.flags.contains(rphp_bytecode::ClassFlags::ANONYMOUS) && stub.linked {
            return Ok(());
        }
        if let Some(&prev) = self.class_index.get(&stub.lname) {
            let p = self.classes[prev as usize].clone();
            let msg = match &p.declared_at {
                Some((file, line)) => format!(
                    "Cannot redeclare class {} (previously declared in {}:{})",
                    p.name_str(),
                    file,
                    line
                ),
                None => format!("Cannot redeclare class {}", p.name_str()),
            };
            return Err(self.fatal_at(&msg, &here_file, here_line));
        }
        let parent = match &decl.parent_name {
            // `extends` autoloads (E7): this is how Composer's PSR-4 loader
            // pulls in a base class declared in another file.
            Some(name) => match self.lookup_class(name)? {
                Some(pid) => Some(pid),
                None => match decl.parent.map(|p| unit.class_base + p) {
                    Some(pid) if self.classes[pid as usize].linked => Some(pid),
                    _ => {
                        let msg = format!(
                            "Class \"{}\" not found",
                            String::from_utf8_lossy(name)
                        );
                        return Err(self.throw_at(crate::ErrorKind::Error, msg, &here_file, here_line));
                    }
                },
            },
            None => match decl.parent.map(|p| unit.class_base + p) {
                Some(pid) if self.classes[pid as usize].linked => Some(pid),
                Some(pid) => {
                    let msg = format!(
                        "Class \"{}\" not found",
                        self.classes[pid as usize].name_str()
                    );
                    return Err(self.throw_at(crate::ErrorKind::Error, msg, &here_file, here_line));
                }
                None => None,
            },
        };
        if parent == Some(id) {
            return Err(self.fatal_at(
                &format!("Class {} has a cyclic inheritance chain", stub.name_str()),
                &here_file,
                here_line,
            ));
        }
        let mut interfaces = Vec::new();
        for iname in &decl.interfaces {
            // `implements` autoloads too.
            match self.lookup_class(iname)? {
                Some(iid) => interfaces.push(iid),
                None => {
                    let msg = format!("Interface \"{}\" not found", String::from_utf8_lossy(iname));
                    return Err(self.throw_at(crate::ErrorKind::Error, msg, &here_file, here_line));
                }
            }
        }
        // E6: a property declaration is an instance slot or a static cell;
        // both carry the declared type, `readonly` and the initializer, which
        // is a folded value or a thunk run in the class's scope.
        let prop_default = |p: &rphp_bytecode::PropDef| match p.default_thunk {
            Some(t) => PropDefault::Thunk(unit.func_base + t),
            None => PropDefault::Value(p.default.clone()),
        };
        // Hook function ids are unit-local in the declaration and
        // process-wide in the class model.
        let rebase = |h: &rphp_bytecode::Hooks| crate::class::PropHooks {
            get: h.get.map(|f| unit.func_base + f),
            set: h.set.map(|f| unit.func_base + f),
        };
        let props = decl
            .props
            .iter()
            .filter(|p| !p.is_static)
            .map(|p| crate::class::PropSpec {
                name: p.name.clone(),
                vis: p.visibility,
                set_vis: p.set_vis,
                ty: p.ty.clone(),
                readonly: p.readonly,
                hooks: p.hooks.as_ref().map(rebase),
                default: prop_default(p),
            })
            .collect();
        let static_props = decl
            .props
            .iter()
            .filter(|p| p.is_static)
            .map(|p| {
                (
                    p.name.clone(),
                    p.visibility,
                    p.ty.clone(),
                    Some(prop_default(p)),
                )
            })
            .collect();
        let consts = decl
            .consts
            .iter()
            .map(|c| crate::class::ConstSpec {
                name: c.name.clone(),
                vis: c.visibility,
                is_final: c.is_final,
                ty: c.ty.clone(),
                init: match c.thunk {
                    Some(t) => PropDefault::Thunk(unit.func_base + t),
                    None => PropDefault::Value(c.value.clone().unwrap_or(Value::Null)),
                },
            })
            .collect();
        let enum_cases = decl
            .enum_cases
            .iter()
            .map(|c| {
                let init = match (c.thunk, &c.value) {
                    (Some(t), _) => crate::class::EnumCaseValue::Pending(unit.func_base + t),
                    (None, Some(v)) => crate::class::EnumCaseValue::Ready(v.clone()),
                    (None, None) => crate::class::EnumCaseValue::None,
                };
                (c.name.clone(), init)
            })
            .collect();
        let enum_backing = match decl.enum_backing {
            rphp_bytecode::EnumBackingType::Int => crate::class::EnumBacking::Int,
            rphp_bytecode::EnumBackingType::String => crate::class::EnumBacking::String,
            // `enum E: int` carries the backing type on the kind as well;
            // either spelling declares a backed enum.
            rphp_bytecode::EnumBackingType::None => match decl.kind {
                rphp_bytecode::ClassKind::Enum {
                    backing: Some(rphp_bytecode::BuiltinType::Int),
                } => crate::class::EnumBacking::Int,
                rphp_bytecode::ClassKind::Enum {
                    backing: Some(rphp_bytecode::BuiltinType::String),
                } => crate::class::EnumBacking::String,
                _ => crate::class::EnumBacking::None,
            },
        };
        let methods = decl
            .methods
            .iter()
            .map(|m| {
                let func = self.funcs[(unit.func_base + m.func) as usize].clone();
                let is_static =
                    m.is_static || func.f.flags.contains(rphp_bytecode::FnFlags::STATIC);
                MethodSpec {
                    name: m.name_bytes.clone(),
                    body: MethodBody::User(func),
                    vis: m.visibility,
                    is_static,
                    is_abstract: m.is_abstract,
                    is_final: m.is_final,
                }
            })
            .collect();
        let spec = ClassSpec {
            name: stub.name.clone(),
            kind: decl.kind,
            flags: decl.flags,
            parent,
            interfaces,
            props,
            methods,
            native_init: None,
            payload_clone: None,
            declared_at: stub.declared_at.clone(),
            internal: false,
            static_props,
            consts,
            enum_cases,
            enum_backing,
        };
        // E6: an enum's implicit members (`name`/`value`, `UnitEnum` /
        // `BackedEnum`, `cases()`/`from()`/`tryFrom()`) are part of the
        // declaration before anything else sees it (`enums.rs`), and traits
        // are copied in before linking, so `link_class` only ever sees own
        // members (`traits.rs`).
        let mut spec = spec;
        if matches!(decl.kind, rphp_bytecode::ClassKind::Enum { .. }) {
            self.enum_implicits(&mut spec);
        }
        self.apply_trait_uses(&mut spec, &decl.traits)?;
        let mut def = match self.link_class(id, spec) {
            Ok(d) => d,
            Err(Unwind::Pending(p)) => {
                // Linking faults are compile-time fatals in php.
                return Err(self.fatal_at(&p.message, &here_file, here_line));
            }
            Err(other) => return Err(other),
        };
        def.unit = Some(unit.clone());
        self.publish_class(def);
        Ok(())
    }

    /// Put a linked definition into the table under its id and name.
    fn publish_class(&mut self, def: ClassDef) {
        let id = def.id;
        let lname = def.lname.clone();
        self.well_known.record(&lname, id);
        if id as usize == self.classes.len() {
            self.classes.push(Rc::new(def));
        } else {
            self.classes[id as usize] = Rc::new(def);
        }
        if !self.class_order.contains(&id) {
            self.class_order.push(id);
        }
        self.class_index.insert(lname, id);
        self.class_gen += 1;
    }

    /// Register a native class from its specification (the
    /// [`ClassBuilder`](crate::ClassBuilder) end point): a re-registered
    /// name keeps its id.
    pub(crate) fn register_class_spec(&mut self, spec: ClassSpec) -> u32 {
        let lname: Box<[u8]> = spec.name.to_ascii_lowercase().into();
        let id = match self.class_index.get(&lname) {
            Some(&id) => id,
            None => self.classes.len() as u32,
        };
        let def = self
            .link_class(id, spec)
            .unwrap_or_else(|u| panic!("native class {}: {}", String::from_utf8_lossy(&lname), u.describe()));
        self.publish_class(def);
        id
    }

    /// An instance of `class` with its default slots and the next object id:
    /// no thunk defaults, no `native_init`, no constructor (the shape
    /// `unserialize` and `(object)` casts need). See [`Interp::new_object`]
    /// for `new`.
    pub fn instantiate(&mut self, class: u32) -> rphp_value::Object {
        let c = self.classes[class as usize].clone();
        let id = self.object_ids.alloc();
        let defaults: Vec<Value> = c
            .props
            .iter()
            .map(|p| match &p.default {
                PropDefault::Value(v) => v.clone(),
                PropDefault::Thunk(_) => Value::Null,
            })
            .collect();
        let obj = rphp_value::Object::new(class, id, c.layout.clone(), defaults);
        if c.magic.contains(MagicFlags::DESTRUCT) {
            obj.add_flags(rphp_value::ObjFlags::HAS_DESTRUCTOR);
            self.destructibles.push(obj.downgrade());
        }
        obj
    }

    /// `new <class>` without running the constructor: rejects abstract
    /// classes, interfaces, traits and enums with php's `Error`, seeds the
    /// slots (thunk defaults evaluated now), runs the `native_init` hook.
    pub fn new_object(&mut self, class: u32) -> Result<rphp_value::Object, Unwind> {
        let c = self.classes[class as usize].clone();
        if !c.is_instantiable() {
            return Err(Unwind::error(format!(
                "Cannot instantiate {} {}",
                c.kind_word(),
                c.name_str()
            )));
        }
        let obj = self.instantiate(class);
        for p in &c.props {
            if let PropDefault::Thunk(fid) = p.default {
                let v = self.run_thunk(fid, None, Some(p.decl))?;
                obj.set_slot(p.slot, v);
            }
        }
        if let Some(init) = c.native_init {
            init(self, &obj)?;
        }
        Ok(obj)
    }

    /// The object's class name for a *message*: php formats one with `%s`,
    /// so an anonymous class shows as a bare `class@anonymous`.
    pub fn class_name_of(&self, o: &rphp_value::Object) -> String {
        let name = &self.classes[o.class_id() as usize].name;
        String::from_utf8_lossy(rphp_value::display_class_name(name)).into_owned()
    }

    /// The object's class name **as php stores it** — the whole thing,
    /// including the part of an anonymous class's name that sits after the
    /// NUL. This is what `get_class()`, `::class` and Reflection answer;
    /// everything that formats a message wants [`Interp::class_name_of`].
    pub fn class_full_name_of(&self, o: &rphp_value::Object) -> Vec<u8> {
        self.classes[o.class_id() as usize].name.to_vec()
    }
}
