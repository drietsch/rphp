//! The runtime class model (plan E4, ADR-021): one [`ClassDef`] serves user
//! and native classes alike. A user class is built from its compiled
//! declaration by [`Interp::declare_class`](crate::Interp::declare_class)
//! (`unit.rs`); a native class through the [`ClassBuilder`](crate::ClassBuilder)
//! in `registry.rs`. Both link parent and interfaces against the process-wide
//! class table, lay out instance properties parent-first (the instance
//! [`Layout`] is built once per class), and flatten the inherited method
//! table so a method lookup is one hash probe.
//!
//! Interfaces are `ClassKind::Interface` definitions carrying method
//! signatures only; `instanceof`, `catch` matching and `is_a` walk `parent`
//! and the flattened `interfaces` ([`Interp::instanceof_class`]).
//!
//! User classes may extend native ones: the slot layout, the `native_init`
//! hook (run at `new`, before the constructor) and the native methods are
//! inherited, so `class MyEx extends Exception` gets `file`/`line`/`trace`
//! filled in and `getMessage()` for free.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use rphp_bytecode::{ClassFlags, ClassKind, TypeDecl, Visibility};
use rphp_value::{Layout, Object, Value};

use crate::registry::{Ctx, NativeResult, Unwind};
use crate::unit::FuncRt;
use crate::Interp;

/// The handler signature of a native method: the interpreter, `$this` (for
/// an instance method), the arguments.
pub type NativeMethodHandler = fn(&mut Ctx, Option<&Object>, &mut [Value]) -> NativeResult;

/// The `native_init` hook of a native class: runs at `new` (after the slots
/// are seeded, before the constructor) on every instance, including
/// instances of user subclasses.
pub type NativeInit = fn(&mut Interp, &Object) -> Result<(), Unwind>;

/// The `payload_clone` hook of a native class: copies the native state of
/// `src` into the freshly made `dst` during `clone`. Without it a class whose
/// state lives in [`Payload`](rphp_value::Payload) — `WeakMap`, the SPL
/// containers — would clone to an *empty* payload, losing its contents
/// silently.
pub type PayloadClone = fn(&mut Interp, &Object, &Object) -> Result<(), Unwind>;

/// A native method descriptor (every field `Copy`, so method tables can be
/// `'static` data).
#[derive(Clone, Copy)]
pub struct NativeMethod {
    /// The implementation.
    pub handler: NativeMethodHandler,
    /// Minimum number of arguments.
    pub min_args: u8,
    /// Maximum number of arguments; `None` means variadic.
    pub max_args: Option<u8>,
    /// Parameter names, for named arguments and messages.
    pub params: &'static [&'static str],
    /// Bitmask of by-reference parameter positions.
    pub by_ref: u32,
    /// `static function`.
    pub is_static: bool,
    /// `final function`.
    pub is_final: bool,
}

impl NativeMethod {
    /// Whether argument position `i` is declared by-reference.
    pub fn is_by_ref(&self, i: usize) -> bool {
        i < 32 && self.by_ref & (1 << i) != 0
    }

    /// Whether `argc` arguments satisfy the declared arity range.
    pub fn accepts(&self, argc: usize) -> bool {
        argc >= self.min_args as usize && self.max_args.is_none_or(|m| argc <= m as usize)
    }

    /// PHP's `ArgumentCountError` text for a call of `Class::method` with
    /// `argc` arguments (`Exception::__construct() expects at most 3
    /// arguments, 4 given`).
    pub fn arity_message(&self, display_name: &str, argc: usize) -> String {
        let min = self.min_args as usize;
        let (kind, n) = match self.max_args {
            Some(max) if max as usize == min => ("exactly", min),
            Some(max) if argc > max as usize => ("at most", max as usize),
            _ => ("at least", min),
        };
        let plural = if n == 1 { "argument" } else { "arguments" };
        format!("{display_name}() expects {kind} {n} {plural}, {argc} given")
    }
}

/// Build a [`NativeMethod`] row: `nm!(0, Some(0), get_message)`.
#[macro_export]
macro_rules! nm {
    ($min:expr, $max:expr, $f:path) => {
        $crate::NativeMethod {
            handler: $f,
            min_args: $min,
            max_args: $max,
            params: &[],
            by_ref: 0,
            is_static: false,
            is_final: false,
        }
    };
}

/// The body of a method: compiled bytecode or a native handler.
pub enum MethodBody {
    /// A user function (a method of a user class, or inherited by one).
    User(Rc<FuncRt>),
    /// A native handler.
    Native(NativeMethod),
}

/// One method of a class (own or inherited).
pub struct MethodDef {
    /// The name as declared (dispatch is case-insensitive).
    pub name: Box<[u8]>,
    /// The implementation.
    pub body: MethodBody,
    /// Visibility.
    pub vis: Visibility,
    /// `static`.
    pub is_static: bool,
    /// `abstract` (or an interface method).
    pub is_abstract: bool,
    /// `final`.
    pub is_final: bool,
    /// The declaring class (process-wide id): the scope for visibility
    /// checks and `self::`.
    pub decl: u32,
}

impl MethodDef {
    /// The user function behind this method, if it is one.
    pub fn user_func(&self) -> Option<&Rc<FuncRt>> {
        match &self.body {
            MethodBody::User(f) => Some(f),
            MethodBody::Native(_) => None,
        }
    }

    /// Whether this is a native method.
    pub fn is_native(&self) -> bool {
        matches!(self.body, MethodBody::Native(_))
    }
}

/// A property default.
#[derive(Clone)]
pub enum PropDefault {
    /// A ready value (cloned into every new instance).
    Value(Value),
    /// A zero-argument thunk (process-wide function id) evaluated at `new`
    /// in the declaring class's scope.
    Thunk(u32),
}

/// One instance property of a class (own or inherited), in slot order.
#[derive(Clone)]
pub struct PropInfo {
    /// Name without the `$`.
    pub name: Box<[u8]>,
    /// The slot index in the instance layout.
    pub slot: u16,
    /// Visibility.
    pub vis: Visibility,
    /// The declaring class (process-wide id).
    pub decl: u32,
    /// Declared type, if any (checked on assignment when E6 lands typed
    /// properties; carried here so the model is complete).
    pub ty: Option<TypeDecl>,
    /// `readonly`.
    pub readonly: bool,
    /// The **write** visibility of an asymmetric declaration
    /// (`public private(set) int $n`), when it is narrower than `vis`.
    /// `readonly` implies `protected(set)` and is handled by `readonly`.
    pub set_vis: Option<Visibility>,
    /// php 8.4 property hooks, as process-wide function ids.
    pub hooks: Option<PropHooks>,
    /// The default value.
    pub default: PropDefault,
    /// The `/** … */` immediately before the declaration.
    pub doc: Option<Box<[u8]>>,
    /// The `#[...]` attributes on the declaration, as the compiler left
    /// them (arguments are unevaluated initializers).
    pub attrs: Vec<rphp_bytecode::AttrDef>,
}

/// A property's `get`/`set` hooks, as **process-wide** function ids (the
/// compiled declaration carries unit-local ones; `unit.rs` rebases them).
#[derive(Clone, Copy, Debug)]
pub struct PropHooks {
    /// `get` hook.
    pub get: Option<u32>,
    /// `set` hook.
    pub set: Option<u32>,
    /// No backing value (see `rphp_bytecode::Hooks::is_virtual`).
    pub is_virtual: bool,
}

/// One own property of a [`ClassSpec`].
pub struct PropSpec {
    /// Name without the `$`.
    pub name: Box<[u8]>,
    /// Read visibility.
    pub vis: Visibility,
    /// Write visibility, when narrower.
    pub set_vis: Option<Visibility>,
    /// Declared type.
    pub ty: Option<TypeDecl>,
    /// `readonly`.
    pub readonly: bool,
    /// Property hooks (process-wide function ids).
    pub hooks: Option<PropHooks>,
    /// The initializer.
    pub default: PropDefault,
    /// The `/** … */` immediately before the declaration.
    pub doc: Option<Box<[u8]>>,
    /// The `#[...]` attributes on the declaration.
    pub attrs: Vec<rphp_bytecode::AttrDef>,
}

impl PropSpec {
    /// A plain property: no type, not readonly, no hooks.
    pub fn new(name: Box<[u8]>, vis: Visibility, default: PropDefault) -> PropSpec {
        PropSpec {
            name,
            vis,
            doc: None,
            attrs: Vec::new(),
            set_vis: None,
            ty: None,
            readonly: false,
            hooks: None,
            default,
        }
    }
}

/// Which magic methods a class defines (own or inherited), so the hot
/// paths (`new`, drops, string conversion, property access) test one bit
/// instead of probing the method table.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct MagicFlags(u32);

impl MagicFlags {
    /// No magic methods.
    pub const NONE: MagicFlags = MagicFlags(0);
    /// `__construct`
    pub const CONSTRUCT: MagicFlags = MagicFlags(1 << 0);
    /// `__destruct`
    pub const DESTRUCT: MagicFlags = MagicFlags(1 << 1);
    /// `__toString`
    pub const TOSTRING: MagicFlags = MagicFlags(1 << 2);
    /// `__get`
    pub const GET: MagicFlags = MagicFlags(1 << 3);
    /// `__set`
    pub const SET: MagicFlags = MagicFlags(1 << 4);
    /// `__isset`
    pub const ISSET: MagicFlags = MagicFlags(1 << 5);
    /// `__unset`
    pub const UNSET: MagicFlags = MagicFlags(1 << 6);
    /// `__call`
    pub const CALL: MagicFlags = MagicFlags(1 << 7);
    /// `__callStatic`
    pub const CALLSTATIC: MagicFlags = MagicFlags(1 << 8);
    /// `__invoke`
    pub const INVOKE: MagicFlags = MagicFlags(1 << 9);
    /// `__clone`
    pub const CLONE: MagicFlags = MagicFlags(1 << 10);
    /// `__serialize`
    pub const SERIALIZE: MagicFlags = MagicFlags(1 << 11);
    /// `__unserialize`
    pub const UNSERIALIZE: MagicFlags = MagicFlags(1 << 12);
    /// `__debugInfo`
    pub const DEBUGINFO: MagicFlags = MagicFlags(1 << 13);

    /// The raw bits.
    pub const fn bits(self) -> u32 {
        self.0
    }

    /// Whether every flag in `other` is set.
    pub const fn contains(self, other: MagicFlags) -> bool {
        self.0 & other.0 == other.0
    }

    /// Set the flags in `other`.
    pub fn insert(&mut self, other: MagicFlags) {
        self.0 |= other.0;
    }
}

impl std::ops::BitOr for MagicFlags {
    type Output = MagicFlags;
    fn bitor(self, rhs: MagicFlags) -> MagicFlags {
        MagicFlags(self.0 | rhs.0)
    }
}

impl std::ops::BitOrAssign for MagicFlags {
    fn bitor_assign(&mut self, rhs: MagicFlags) {
        self.0 |= rhs.0;
    }
}

impl std::fmt::Debug for MagicFlags {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "MagicFlags({:#x})", self.0)
    }
}

impl MagicFlags {
    /// The flag a (lowercased) method name sets, if it is a magic method.
    pub fn of_method(lname: &[u8]) -> MagicFlags {
        match lname {
            b"__construct" => MagicFlags::CONSTRUCT,
            b"__destruct" => MagicFlags::DESTRUCT,
            b"__tostring" => MagicFlags::TOSTRING,
            b"__get" => MagicFlags::GET,
            b"__set" => MagicFlags::SET,
            b"__isset" => MagicFlags::ISSET,
            b"__unset" => MagicFlags::UNSET,
            b"__call" => MagicFlags::CALL,
            b"__callstatic" => MagicFlags::CALLSTATIC,
            b"__invoke" => MagicFlags::INVOKE,
            b"__clone" => MagicFlags::CLONE,
            b"__serialize" => MagicFlags::SERIALIZE,
            b"__unserialize" => MagicFlags::UNSERIALIZE,
            b"__debuginfo" => MagicFlags::DEBUGINFO,
            _ => MagicFlags::NONE,
        }
    }
}

/// One **static** property of a class, own or inherited.
///
/// The value lives in a shared cell: a subclass that does not redeclare the
/// property inherits the *same* `Rc`, so `B::$n` and `A::$n` are one storage
/// location (php's semantics). Redeclaring in a subclass creates a new cell.
///
/// The initializer is evaluated lazily on first access (`statics.rs`) rather
/// than at link time, so linking never runs user code.
pub struct StaticPropInfo {
    /// Name without the `$`.
    pub name: Box<[u8]>,
    /// Visibility.
    pub vis: Visibility,
    /// The declaring class (process-wide id).
    pub decl: u32,
    /// Declared type, if any.
    pub ty: Option<TypeDecl>,
    /// The shared storage. `Value::Uninit` until `init` has been evaluated.
    pub cell: Rc<RefCell<Value>>,
    /// The declared initializer, consumed on first access.
    pub init: Option<PropDefault>,
    /// Whether `init` has been evaluated into `cell`.
    pub ready: Rc<std::cell::Cell<bool>>,
}

impl Clone for StaticPropInfo {
    fn clone(&self) -> StaticPropInfo {
        StaticPropInfo {
            name: self.name.clone(),
            vis: self.vis,
            decl: self.decl,
            ty: self.ty.clone(),
            cell: self.cell.clone(),
            init: self.init.clone(),
            ready: self.ready.clone(),
        }
    }
}

/// The evaluation state of a class constant or enum case value.
pub enum ConstState {
    /// Not evaluated yet.
    Pending(PropDefault),
    /// Being evaluated — a second visit is a cycle.
    Evaluating,
    /// Evaluated.
    Ready(Value),
}

/// One class constant (own or inherited), evaluated lazily in the declaring
/// class's scope with cycle detection.
pub struct ClassConst {
    /// Declared name (case-sensitive).
    pub name: Box<[u8]>,
    /// Visibility.
    pub vis: Visibility,
    /// The declaring class (process-wide id).
    pub decl: u32,
    /// `final`.
    pub is_final: bool,
    /// Declared type (8.3 typed constants), if any.
    pub ty: Option<TypeDecl>,
    /// Lazy value.
    pub state: RefCell<ConstState>,
    /// php deprecates the constant: the text after `Constant C::X is
    /// deprecated`, raised on every fetch.
    pub deprecated: Option<&'static str>,
}

/// What an enum's cases are backed by.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EnumBacking {
    /// A pure enum: cases have no value.
    None,
    /// `enum E: int`.
    Int,
    /// `enum E: string`.
    String,
}

/// One enum case. The case object is a singleton, materialized on first use
/// and then shared by every `E::Case` evaluation (`enums.rs`).
pub struct EnumCaseInfo {
    /// Case name.
    pub name: Box<[u8]>,
    /// The backing value: absent for a pure enum, a folded literal, or an
    /// initializer expression not yet run (see [`EnumCaseValue`]).
    pub value: RefCell<EnumCaseValue>,
    /// The singleton instance, once materialized.
    pub instance: RefCell<Option<Value>>,
}

/// An enum case's backing value, which php requires to be a constant
/// expression but not a literal: `case A = 1 << 0;` and `case B = self::X;`
/// are initializers, evaluated in the enum's own scope the first time
/// anything asks for the case.
#[derive(Clone)]
pub enum EnumCaseValue {
    /// A pure enum's case has none.
    None,
    /// An initializer thunk (process-wide function id), not run yet.
    Pending(u32),
    /// Evaluated, or a literal from the start.
    Ready(Value),
}

/// A class as the runtime holds it (user or native), linked: parent and
/// interfaces resolved, properties laid out, methods flattened.
pub struct ClassDef {
    /// Process-wide id (the index in the interpreter's class table).
    pub id: u32,
    /// Declared name (original case, FQN without a leading `\`).
    pub name: Box<[u8]>,
    /// Lowercased `name` (the class-table key).
    pub lname: Box<[u8]>,
    /// Class / interface / trait / enum.
    pub kind: ClassKind,
    /// Parent class (process-wide id).
    pub parent: Option<u32>,
    /// Every interface the class implements, flattened over the parent
    /// chain and the interfaces' own parents.
    pub interfaces: Vec<u32>,
    /// Modifiers.
    pub flags: ClassFlags,
    /// Instance properties in slot order (parent-first).
    pub props: Vec<PropInfo>,
    /// Property name → index into `props`.
    pub prop_index: HashMap<Box<[u8]>, u16>,
    /// Lowercased method name → method (own and inherited).
    pub methods: HashMap<Box<[u8]>, Rc<MethodDef>>,
    /// Method names in php's `get_class_methods` order: own methods in
    /// declaration order, then inherited ones.
    pub method_order: Vec<Box<[u8]>>,
    /// The magic methods defined (own or inherited).
    pub magic: MagicFlags,
    /// The native initialization hook (own or inherited).
    pub native_init: Option<NativeInit>,
    /// The native payload-copy hook used by `clone` (own or inherited).
    pub payload_clone: Option<PayloadClone>,
    /// The instance layout, shared by every instance.
    pub layout: Rc<Layout>,
    /// The unit that declared the class and the declaration line, for
    /// `Cannot redeclare class X (previously declared in file:line)`;
    /// `None` for native classes.
    pub declared_at: Option<(Rc<str>, u32)>,
    /// Whether the class is linked (declared). A stub for a not-yet-declared
    /// class of a loaded unit has this `false` and no members.
    pub linked: bool,
    /// `true` for classes registered by the engine / an extension.
    pub internal: bool,
    /// The `/** … */` immediately before the declaration, which
    /// `ReflectionClass::getDocComment()` answers with.
    pub doc: Option<Box<[u8]>>,
    /// The `#[...]` attributes on the declaration, which
    /// `ReflectionClass::getAttributes()` answers with.
    pub attrs: Vec<rphp_bytecode::AttrDef>,
    /// The unit that compiled the class (user classes), for `declare_class`.
    pub unit: Option<Rc<crate::unit::UnitRt>>,
    /// Static properties in declaration order (own and inherited).
    pub static_props: Vec<StaticPropInfo>,
    /// Static property name → index into `static_props`.
    pub static_index: HashMap<Box<[u8]>, u16>,
    /// Class constants by name (own, inherited and from interfaces).
    pub consts: HashMap<Box<[u8]>, Rc<ClassConst>>,
    /// Constant names in declaration order (own first, then inherited).
    pub const_order: Vec<Box<[u8]>>,
    /// Enum cases in declaration order (empty for non-enums).
    pub enum_cases: Vec<Rc<EnumCaseInfo>>,
    /// Enum case name → index into `enum_cases`.
    pub enum_index: HashMap<Box<[u8]>, u16>,
    /// Whether the backed-enum lookup table has been built. php builds it the
    /// first time anything touches a case, which is when it evaluates the
    /// case initializers and rejects a duplicate value — not at declaration.
    pub enum_table_built: std::cell::Cell<bool>,
    /// What the enum's cases are backed by (`None` for non-enums too).
    pub enum_backing: EnumBacking,
    /// The traits this class itself `use`s, in declaration order. php's
    /// `class_uses()` reports a class's *own* traits only.
    pub used_traits: Vec<u32>,
}

impl ClassDef {
    /// A stub for a class that a unit declares later (`DeclareClass`).
    pub(crate) fn stub(id: u32, name: &[u8], kind: ClassKind, flags: ClassFlags, declared_at: Option<(Rc<str>, u32)>) -> ClassDef {
        ClassDef {
            id,
            name: Box::from(name),
            lname: name.to_ascii_lowercase().into_boxed_slice(),
            kind,
            parent: None,
            interfaces: Vec::new(),
            flags,
            props: Vec::new(),
            prop_index: HashMap::new(),
            methods: HashMap::new(),
            method_order: Vec::new(),
            magic: MagicFlags::NONE,
            native_init: None,
            payload_clone: None,
            layout: Rc::new(Layout::empty(Rc::from(name))),
            declared_at,
            linked: false,
            internal: false,
            doc: None,
            attrs: Vec::new(),
            unit: None,
            static_props: Vec::new(),
            static_index: HashMap::new(),
            consts: HashMap::new(),
            const_order: Vec::new(),
            enum_cases: Vec::new(),
            enum_table_built: std::cell::Cell::new(false),
            enum_index: HashMap::new(),
            enum_backing: EnumBacking::None,
            used_traits: Vec::new(),
        }
    }

    /// The class name as a message shows it: php formats one with `%s`, so an
    /// anonymous class is a bare `class@anonymous` everywhere but in
    /// `get_class()`, `::class`, `var_export` and Reflection, which read
    /// [`ClassDef::name`] itself.
    pub fn name_str(&self) -> String {
        String::from_utf8_lossy(rphp_value::display_class_name(&self.name)).into_owned()
    }

    /// The method with this (case-insensitive) name, own or inherited.
    pub fn method(&self, name: &[u8]) -> Option<&Rc<MethodDef>> {
        if name.iter().any(u8::is_ascii_uppercase) {
            self.methods.get(name.to_ascii_lowercase().as_slice())
        } else {
            self.methods.get(name)
        }
    }

    /// The declared instance property `name`.
    pub fn prop(&self, name: &[u8]) -> Option<&PropInfo> {
        self.prop_index.get(name).map(|&i| &self.props[i as usize])
    }

    /// Whether the class can be instantiated (`new`).
    pub fn is_instantiable(&self) -> bool {
        self.kind == ClassKind::Class && !self.flags.contains(ClassFlags::ABSTRACT)
    }

    /// Whether instances may grow dynamic properties without the 8.2
    /// deprecation (`stdClass`, `#[AllowDynamicProperties]`).
    pub fn allows_dynamic_props(&self) -> bool {
        self.flags.contains(ClassFlags::ALLOW_DYNAMIC)
    }

    /// The kind word for `Cannot instantiate <kind> X` messages.
    pub fn kind_word(&self) -> &'static str {
        match self.kind {
            ClassKind::Interface => "interface",
            ClassKind::Trait => "trait",
            ClassKind::Enum { .. } => "enum",
            ClassKind::Class => {
                if self.flags.contains(ClassFlags::ABSTRACT) {
                    "abstract class"
                } else {
                    "class"
                }
            }
        }
    }
}

/// Everything a class declaration needs before linking: the names of the
/// parent and interfaces, the own members. Built from a compiled `Class`
/// (E3 `Module`) or from the native builder; E6 builds it from `ClassDecl`.
pub struct ClassSpec {
    /// Declared name.
    pub name: Box<[u8]>,
    /// Class / interface / trait / enum.
    pub kind: ClassKind,
    /// Modifiers.
    pub flags: ClassFlags,
    /// `extends` (already resolved to a process-wide id).
    pub parent: Option<u32>,
    /// `implements` (already resolved).
    pub interfaces: Vec<u32>,
    /// Own instance properties in declaration order.
    pub props: Vec<PropSpec>,
    /// Own methods in declaration order.
    pub methods: Vec<MethodSpec>,
    /// The native init hook (own).
    pub native_init: Option<NativeInit>,
    /// The native payload-copy hook for `clone` (own).
    pub payload_clone: Option<PayloadClone>,
    /// Where it was declared.
    pub declared_at: Option<(Rc<str>, u32)>,
    /// Registered by the engine / an extension.
    pub internal: bool,
    /// Own static properties: name, visibility, type, initializer.
    pub static_props: Vec<(Box<[u8]>, Visibility, Option<TypeDecl>, Option<PropDefault>)>,
    /// Own class constants, in declaration order.
    pub consts: Vec<ConstSpec>,
    /// Own enum cases, in declaration order (name, backing initializer).
    pub enum_cases: Vec<(Box<[u8]>, EnumCaseValue)>,
    /// What this enum's cases are backed by.
    pub enum_backing: EnumBacking,
    /// The traits the declaration `use`s, resolved to ids by
    /// [`Interp::apply_trait_uses`] before linking (`class_uses()`).
    pub used_traits: Vec<u32>,
    /// The `/** … */` immediately before the declaration.
    pub doc: Option<Box<[u8]>>,
    /// The `#[...]` attributes on the declaration.
    pub attrs: Vec<rphp_bytecode::AttrDef>,
}

/// One own class constant of a [`ClassSpec`].
pub struct ConstSpec {
    /// Declared name.
    pub name: Box<[u8]>,
    /// Visibility.
    pub vis: Visibility,
    /// `final`.
    pub is_final: bool,
    /// Declared type (8.3), if any.
    pub ty: Option<TypeDecl>,
    /// The initializer, evaluated on first use.
    pub init: PropDefault,
    /// See [`ClassConst::deprecated`].
    pub deprecated: Option<&'static str>,
}

/// One own method of a [`ClassSpec`].
pub struct MethodSpec {
    /// Declared name.
    pub name: Box<[u8]>,
    /// The body.
    pub body: MethodBody,
    /// Visibility.
    pub vis: Visibility,
    /// `static`.
    pub is_static: bool,
    /// `abstract`.
    pub is_abstract: bool,
    /// `final`.
    pub is_final: bool,
}

impl Interp {
    /// Link a class specification into a [`ClassDef`] with id `id`: inherit
    /// the parent's layout, methods, magic flags and native init; append the
    /// own properties (a redeclared property keeps its parent slot and takes
    /// the new visibility/default) and methods (an override replaces the
    /// inherited entry). Interfaces are flattened. The result is not yet in
    /// the class table.
    pub(crate) fn link_class(&self, id: u32, spec: ClassSpec) -> Result<ClassDef, Unwind> {
        let ClassSpec {
            used_traits,
            doc,
            attrs,
            name,
            kind,
            flags,
            parent,
            interfaces,
            props,
            methods,
            native_init,
            payload_clone,
            declared_at,
            internal,
            static_props,
            consts,
            enum_cases,
            enum_backing,
        } = spec;
        let name_str = String::from_utf8_lossy(&name).into_owned();
        let mut def = ClassDef::stub(id, &name, kind, flags, declared_at);
        def.internal = internal;
        def.doc = doc;
        def.attrs = attrs;
        def.linked = true;
        if let Some(pid) = parent {
            let p = self.classes[pid as usize].clone();
            if !p.linked {
                return Err(Unwind::error(format!(
                    "Class \"{}\" not found",
                    p.name_str()
                )));
            }
            if p.kind == ClassKind::Interface && kind != ClassKind::Interface {
                return Err(Unwind::error(format!(
                    "Class {name_str} cannot extend interface {}",
                    p.name_str()
                )));
            }
            if p.kind == ClassKind::Trait {
                return Err(Unwind::error(format!(
                    "Class {name_str} cannot extend trait {}",
                    p.name_str()
                )));
            }
            if p.flags.contains(ClassFlags::FINAL) {
                return Err(Unwind::error(format!(
                    "Class {name_str} cannot extend final class {}",
                    p.name_str()
                )));
            }
            def.parent = Some(pid);
            def.interfaces = p.interfaces.clone();
            def.props = p.props.clone();
            def.prop_index = p.prop_index.clone();
            for (k, m) in &p.methods {
                def.methods.insert(k.clone(), m.clone());
            }
            def.magic = p.magic;
            def.native_init = p.native_init;
            def.payload_clone = p.payload_clone;
            // Static properties are inherited by *sharing* the parent's cell:
            // `B::$n` and `A::$n` are one location unless B redeclares it.
            def.static_props = p.static_props.clone();
            def.static_index = p.static_index.clone();
            for (k, c) in &p.consts {
                def.consts.insert(k.clone(), c.clone());
            }
            def.const_order = p.const_order.clone();
            if p.allows_dynamic_props() {
                def.flags |= ClassFlags::ALLOW_DYNAMIC;
            }
        }
        for iid in interfaces {
            let i = self.classes[iid as usize].clone();
            if i.kind != ClassKind::Interface {
                return Err(Unwind::error(format!(
                    "{name_str} cannot implement {} - it is not an interface",
                    i.name_str()
                )));
            }
            for &sub in &i.interfaces {
                if !def.interfaces.contains(&sub) {
                    def.interfaces.push(sub);
                }
            }
            if !def.interfaces.contains(&iid) {
                def.interfaces.push(iid);
            }
            // Interface methods are abstract signatures: inherit them so
            // `method_exists` and Reflection see them; a class body's own
            // method replaces the entry below.
            for (k, m) in &i.methods {
                def.methods.entry(k.clone()).or_insert_with(|| m.clone());
            }
            // Interface constants are inherited by the implementing class.
            for (k, c) in &i.consts {
                if !def.consts.contains_key(k) {
                    def.consts.insert(k.clone(), c.clone());
                    def.const_order.push(k.clone());
                }
            }
        }
        for ps in props {
            // Re-declaring an ancestor's **private** property does not
            // replace it: php keeps both slots on the object, the
            // ancestor's under its mangled key, reachable by the plain name
            // from that ancestor's own code only. The layout indexes it the
            // same way (`Layout::new`).
            let inherited_private = def
                .prop_index
                .get(&ps.name)
                .map(|&i| &def.props[i as usize])
                .is_some_and(|p| p.vis == Visibility::Private && p.decl != id);
            if inherited_private {
                let i = def.prop_index.remove(&ps.name).expect("looked up just above");
                let decl_name = self.classes[def.props[i as usize].decl as usize].name.clone();
                def.prop_index
                    .insert(rphp_value::mangled_key(&decl_name, &ps.name), i);
            }
            match def.prop_index.get(&ps.name).copied() {
                Some(i) => {
                    let p = &mut def.props[i as usize];
                    p.vis = ps.vis;
                    p.decl = id;
                    p.ty = ps.ty;
                    p.readonly = ps.readonly;
                    p.set_vis = ps.set_vis;
                    p.hooks = ps.hooks;
                    p.default = ps.default;
                }
                None => {
                    let slot = def.props.len() as u16;
                    def.prop_index.insert(ps.name.clone(), slot);
                    def.props.push(PropInfo {
                        name: ps.name,
                        slot,
                        vis: ps.vis,
                        decl: id,
                        ty: ps.ty,
                        readonly: ps.readonly,
                        set_vis: ps.set_vis,
                        hooks: ps.hooks,
                        default: ps.default,
                        doc: ps.doc,
                        attrs: ps.attrs,
                    });
                }
            }
        }
        for (sname, vis, ty, init) in static_props {
            let info = StaticPropInfo {
                name: sname.clone(),
                vis,
                decl: id,
                ty,
                cell: Rc::new(RefCell::new(Value::Uninit)),
                init,
                ready: Rc::new(std::cell::Cell::new(false)),
            };
            match def.static_index.get(&sname).copied() {
                // A redeclaration in a subclass gets its own cell.
                Some(i) => def.static_props[i as usize] = info,
                None => {
                    let slot = def.static_props.len() as u16;
                    def.static_index.insert(sname, slot);
                    def.static_props.push(info);
                }
            }
        }
        let mut own_consts: Vec<Box<[u8]>> = Vec::new();
        for c in consts {
            if let Some(prev) = def.consts.get(&c.name) {
                if prev.is_final && prev.decl != id {
                    return Err(Unwind::error(format!(
                        "{name_str}::{} cannot override final constant {}::{}",
                        String::from_utf8_lossy(&c.name),
                        self.classes[prev.decl as usize].name_str(),
                        String::from_utf8_lossy(&prev.name)
                    )));
                }
            } else {
                own_consts.push(c.name.clone());
            }
            def.consts.insert(
                c.name.clone(),
                Rc::new(ClassConst {
                    name: c.name,
                    vis: c.vis,
                    decl: id,
                    is_final: c.is_final,
                    ty: c.ty,
                    state: RefCell::new(ConstState::Pending(c.init)),
                    deprecated: c.deprecated,
                }),
            );
        }
        // Own constants come first in `get_class_constants` order.
        own_consts.extend(std::mem::take(&mut def.const_order));
        def.const_order = own_consts;
        def.enum_backing = enum_backing;
        def.used_traits = used_traits;
        for (cname, value) in enum_cases {
            let slot = def.enum_cases.len() as u16;
            def.enum_index.insert(cname.clone(), slot);
            def.enum_cases.push(Rc::new(EnumCaseInfo {
                name: cname,
                value: RefCell::new(value),
                instance: RefCell::new(None),
            }));
        }
        let mut own_order: Vec<Box<[u8]>> = Vec::new();
        for m in methods {
            let key: Box<[u8]> = m.name.to_ascii_lowercase().into_boxed_slice();
            if let Some(prev) = def.methods.get(&key) {
                if prev.is_final && prev.decl != id && !prev.is_abstract {
                    return Err(Unwind::error(format!(
                        "Cannot override final method {}::{}()",
                        self.classes[prev.decl as usize].name_str(),
                        String::from_utf8_lossy(&prev.name)
                    )));
                }
            }
            def.magic |= MagicFlags::of_method(&key);
            own_order.push(key.clone());
            def.methods.insert(
                key,
                Rc::new(MethodDef {
                    name: m.name,
                    body: m.body,
                    vis: m.vis,
                    is_static: m.is_static,
                    is_abstract: m.is_abstract,
                    is_final: m.is_final,
                    decl: id,
                }),
            );
        }
        if let Some(f) = native_init {
            def.native_init = Some(f);
        }
        if let Some(f) = payload_clone {
            def.payload_clone = Some(f);
        }
        // php implicitly implements `Stringable` for any class that declares
        // `__toString()`, so `$o instanceof Stringable` and a `Stringable`
        // parameter type both accept it without the class saying so.
        if def.magic.contains(MagicFlags::TOSTRING) {
            if let Some(sid) = self.well_known.stringable {
                if def.id != sid && !def.interfaces.contains(&sid) {
                    def.interfaces.push(sid);
                }
            }
        }
        // Method order: own methods first, then the parent's order.
        let mut order = own_order;
        if let Some(pid) = def.parent {
            for k in &self.classes[pid as usize].method_order {
                if !order.contains(k) {
                    order.push(k.clone());
                }
            }
        }
        for iid in &def.interfaces {
            for k in &self.classes[*iid as usize].method_order {
                if !order.contains(k) {
                    order.push(k.clone());
                }
            }
        }
        def.method_order = order;
        // The instance layout.
        let metas: Vec<rphp_value::PropMeta> = def
            .props
            .iter()
            .map(|p| rphp_value::PropMeta {
                name: p.name.clone(),
                vis: match p.vis {
                    Visibility::Public => rphp_value::Vis::Public,
                    Visibility::Protected => rphp_value::Vis::Protected,
                    Visibility::Private => rphp_value::Vis::Private,
                },
                decl_class: p.decl,
                decl_class_name: Rc::from(&self.class_display_name(p.decl, id, &name)[..]),
                ty: p.ty.as_ref().map(|t| Rc::from(t.to_string().as_str())),
                is_virtual: p.hooks.is_some_and(|h| h.is_virtual),
            })
            .collect();
        def.layout = Rc::new(Layout::new(Rc::from(&name[..]), metas));
        Ok(def)
    }

    /// The name of class `cid`, or `own_name` when `cid` is the class being
    /// linked (not in the table yet).
    fn class_display_name(&self, cid: u32, linking: u32, own_name: &[u8]) -> Vec<u8> {
        if cid == linking {
            own_name.to_vec()
        } else {
            self.classes[cid as usize].name.to_vec()
        }
    }

    /// Whether `class` is `target`, descends from it, or implements it.
    pub fn instanceof_class(&self, class: u32, target: u32) -> bool {
        let mut cur = Some(class);
        while let Some(cid) = cur {
            if cid == target {
                return true;
            }
            let c = &self.classes[cid as usize];
            if c.interfaces.contains(&target) {
                return true;
            }
            cur = c.parent;
        }
        false
    }

    /// Whether the object is an instance of `target` (class or interface).
    pub fn object_instanceof(&self, o: &Object, target: u32) -> bool {
        self.instanceof_class(o.class_id(), target)
    }

    /// Whether the object is a `Throwable`.
    pub fn is_throwable(&self, o: &Object) -> bool {
        match self.well_known.throwable {
            Some(t) => self.object_instanceof(o, t),
            None => false,
        }
    }

    /// The runtime class of an object.
    pub fn class_of(&self, o: &Object) -> &Rc<ClassDef> {
        &self.classes[o.class_id() as usize]
    }
}

/// Process-wide ids of the classes the engine itself needs to find (filled
/// by the registry as the extensions declare them; `None` until then).
#[derive(Clone, Copy, Default, Debug)]
pub struct WellKnown {
    /// `stdClass`.
    pub stdclass: Option<u32>,
    /// `Stringable`.
    pub stringable: Option<u32>,
    /// `Throwable`.
    pub throwable: Option<u32>,
    /// `Exception`.
    pub exception: Option<u32>,
    /// `Error`.
    pub error: Option<u32>,
    /// `ErrorException`.
    pub error_exception: Option<u32>,
    /// `TypeError`.
    pub type_error: Option<u32>,
    /// `ValueError`.
    pub value_error: Option<u32>,
    /// `ArgumentCountError`.
    pub argument_count_error: Option<u32>,
    /// `ArithmeticError`.
    pub arithmetic_error: Option<u32>,
    /// `DivisionByZeroError`.
    pub division_by_zero_error: Option<u32>,
    /// `UnhandledMatchError`.
    pub unhandled_match_error: Option<u32>,
    /// `AssertionError`.
    pub assertion_error: Option<u32>,
    /// `Closure`.
    pub closure: Option<u32>,
    /// `Traversable` (the marker interface `foreach` tests).
    pub traversable: Option<u32>,
    /// `Iterator`.
    pub iterator: Option<u32>,
    /// `IteratorAggregate`.
    pub iterator_aggregate: Option<u32>,
    /// `UnitEnum`, implicitly implemented by every enum.
    pub unit_enum: Option<u32>,
    /// `BackedEnum`, implicitly implemented by every backed enum.
    pub backed_enum: Option<u32>,
    /// `ArrayAccess`, for `$o[$k]` (E6 object protocols).
    pub array_access: Option<u32>,
    /// `Countable`, for `count($o)`.
    pub countable: Option<u32>,
    /// `Generator` (E8).
    pub generator: Option<u32>,
}

impl WellKnown {
    /// Record a registered class under its well-known slot, if it has one.
    pub(crate) fn record(&mut self, lname: &[u8], id: u32) {
        let slot = match lname {
            b"stdclass" => &mut self.stdclass,
            b"stringable" => &mut self.stringable,
            b"throwable" => &mut self.throwable,
            b"exception" => &mut self.exception,
            b"error" => &mut self.error,
            b"errorexception" => &mut self.error_exception,
            b"typeerror" => &mut self.type_error,
            b"valueerror" => &mut self.value_error,
            b"argumentcounterror" => &mut self.argument_count_error,
            b"arithmeticerror" => &mut self.arithmetic_error,
            b"divisionbyzeroerror" => &mut self.division_by_zero_error,
            b"unhandledmatcherror" => &mut self.unhandled_match_error,
            b"assertionerror" => &mut self.assertion_error,
            b"closure" => &mut self.closure,
            b"traversable" => &mut self.traversable,
            b"iterator" => &mut self.iterator,
            b"iteratoraggregate" => &mut self.iterator_aggregate,
            b"unitenum" => &mut self.unit_enum,
            b"backedenum" => &mut self.backed_enum,
            b"arrayaccess" => &mut self.array_access,
            b"countable" => &mut self.countable,
            b"generator" => &mut self.generator,
            _ => return,
        };
        *slot = Some(id);
    }
}
