//! Class-like lowering: the declaration pre-pass (every class-like in the unit
//! gets a [`ClassId`] up front so `extends` can name a class declared later or
//! conditionally) and the `rphp_bytecode::Class` assembly.
//!
//! Every named `class` / `interface` / `trait` / `enum` declaration is lowered.
//! A [`BcClass`] is the whole declaration as written, *unlinked*: parent and
//! interface names, the `use T { ... }` statements verbatim, own properties
//! (instance and static, with their declared type, `readonly`, asymmetric
//! `set` visibility and hooks), own methods (including `static`, `abstract`
//! and interface signatures), own class constants and own enum cases. The
//! runtime resolves the names, copies trait members in and lays out the
//! instance at declaration time; nothing here runs user code.
//!
//! Initializers — property defaults, class constants, parameter defaults — are
//! a folded [`Value`] when they are literal and a zero-argument **thunk**
//! function otherwise, so `public array $a = [self::X];` and `const B = A * 2;`
//! are evaluated lazily, in the declaring class's scope, on first use rather
//! than while the class is being linked. A property with a declared type and no
//! initializer starts [`Value::Uninit`], php's "uninitialized" state.
//!
//! Constructor property promotion is lowered in both halves: the [`PropDef`]
//! is added here, from the constructor's parameter list, and the assignment to
//! `$this->x` is emitted by [`crate::func::FnCompiler::compile_params`].
//!
//! Anonymous classes (`new class { ... }`) are the one class-like form still
//! reported as `RPHP_E0300`; they are numbered and instantiated from the
//! expression side.

use std::collections::HashMap;

use rphp_ast::v2::visit::{walk_expr, walk_stmt, Visitor};
use rphp_ast::v2::{
    Adaptation, ArrayItem, Builtin, ClassKind, ClassLike, ConstMember, EnumCase, Expr, Hook,
    HookKind, Member, MethodDecl, Param, Program, PropMember, Stmt, TraitUse, Type, TypeKind, UnOp,
    Visibility as AstVis,
};
use rphp_bytecode::{
    BuiltinType, Class as BcClass, ClassConstDef, ClassFlags, ClassId, ClassKind as BcClassKind,
    EnumBackingType, EnumCaseDef, FuncId, Hooks, Method as BcMethod, PropDef,
    TraitAdaptation as BcAdaptation, TraitUse as BcTraitUse, Visibility,
};
use rphp_diagnostics::Diagnostic;
use rphp_intern::{IdentId, Interner};
use rphp_span::Span;
use rphp_value::{array_key, Array, Str, Value};

use crate::func::{
    compile_function, compile_hook, compile_thunk_in, lower_type_at, FnSpec, ModuleCtx,
};
use crate::{unsupported, NON_CONST_PROP_DEFAULT, REDECLARED_CLASS};

/// Lowercased class name → pre-assigned id.
pub(crate) type ClassMap = HashMap<Box<[u8]>, ClassId>;
/// Class-like declaration node (by address) → pre-assigned id.
pub(crate) type ClassIds = HashMap<*const ClassLike, ClassId>;

/// Assign a [`ClassId`] to every named class-like declaration in the unit,
/// wherever it appears (top level, blocks, function bodies, conditionals),
/// in source order. Returns the lowercased-name map (first declaration wins
/// for a name declared twice conditionally) and the per-node id map.
pub(crate) fn collect_class_ids(program: &Program, interner: &Interner) -> (ClassMap, ClassIds) {
    let mut v = ClassCollector {
        interner,
        map: HashMap::new(),
        ids: HashMap::new(),
    };
    v.visit_program(program);
    (v.map, v.ids)
}

struct ClassCollector<'a> {
    interner: &'a Interner,
    map: HashMap<Box<[u8]>, ClassId>,
    ids: HashMap<*const ClassLike, ClassId>,
}

impl Visitor for ClassCollector<'_> {
    fn visit_stmt(&mut self, s: &Stmt) {
        if let Stmt::ClassLike(c) = s {
            if let Some(name) = c.name {
                let id = self.ids.len() as ClassId;
                self.ids.insert(c as *const ClassLike, id);
                let key: Box<[u8]> = self.interner.resolve(name).to_ascii_lowercase().into();
                self.map.entry(key).or_insert(id);
            }
        }
        walk_stmt(self, s);
    }

    fn visit_expr(&mut self, e: &Expr) {
        // Anonymous classes are not lowered; nothing to number inside them.
        walk_expr(self, e);
    }
}

/// Compile one class-like declaration into the module's class table at its
/// pre-assigned id (methods, hooks and initializer thunks go to the function
/// sink). `None` when the declaration is anonymous, which is not lowered yet.
pub(crate) fn compile_class(
    mx: &ModuleCtx<'_>,
    diags: &mut Vec<Diagnostic>,
    c: &ClassLike,
) -> Option<ClassId> {
    let interner = mx.interner;
    let Some(name) = c.name else {
        unsupported(diags, c.span, "anonymous class");
        return None;
    };
    let id = *mx
        .class_ids
        .get(&(c as *const ClassLike))
        .expect("class numbered by the pre-pass");

    let mut flags = ClassFlags::NONE;
    if c.modifiers.abstract_ {
        flags |= ClassFlags::ABSTRACT;
    }
    if c.modifiers.final_ {
        flags |= ClassFlags::FINAL;
    }
    if c.modifiers.readonly {
        // Every instance property of a `readonly class` is readonly; the HIR
        // pass has already set the modifier on each of them (and on the
        // promoted constructor parameters), so only the class flag is left.
        flags |= ClassFlags::READONLY;
    }
    let (kind, enum_backing) = match c.kind {
        ClassKind::Class => (BcClassKind::Class, EnumBackingType::None),
        ClassKind::Interface => {
            // Not instantiable, like the native interfaces the registry builds.
            flags |= ClassFlags::ABSTRACT;
            (BcClassKind::Interface, EnumBackingType::None)
        }
        ClassKind::Trait => {
            flags |= ClassFlags::ABSTRACT;
            (BcClassKind::Trait, EnumBackingType::None)
        }
        ClassKind::Enum => {
            // php makes every enum final: `class X extends E` is a fatal.
            flags |= ClassFlags::FINAL;
            let backing = enum_backing_of(c, diags);
            let kw = match backing {
                EnumBackingType::None => None,
                EnumBackingType::Int => Some(BuiltinType::Int),
                EnumBackingType::String => Some(BuiltinType::String),
            };
            (BcClassKind::Enum { backing: kw }, backing)
        }
    };

    // An interface's `extends` list is its interface list; a class or enum has
    // at most one parent, carried by name (the runtime resolves it against its
    // class table, so native and cross-unit parents work) plus the unit-local
    // id as a compile-time hint.
    let mut interfaces: Vec<Box<[u8]>> = Vec::new();
    let (parent, parent_name) = if c.kind == ClassKind::Interface {
        interfaces.extend(
            c.extends
                .iter()
                .map(|i| class_fqn(i, interner).into_boxed_slice()),
        );
        (None, None)
    } else {
        match c.extends.as_slice() {
            [] => (None, None),
            [p] => {
                let fqn = class_fqn(p, interner);
                let key = fqn.to_ascii_lowercase();
                (
                    mx.class_map.get(key.as_slice()).copied(),
                    Some(fqn.into_boxed_slice()),
                )
            }
            [_, second, ..] => {
                unsupported(diags, second.span, "multiple parents on a class");
                (None, None)
            }
        }
    };
    if parent == Some(id) {
        diags.push(
            Diagnostic::error(
                REDECLARED_CLASS,
                format!(
                    "class \"{}\" has a cyclic inheritance chain",
                    interner.resolve_lossy(name)
                ),
            )
            .with_primary(c.span, "cyclic `extends`"),
        );
    }
    interfaces.extend(
        c.implements
            .iter()
            .map(|i| class_fqn(i, interner).into_boxed_slice()),
    );

    let mut lo = MemberLower {
        mx,
        diags,
        scope: (id, name),
        props: Vec::new(),
        methods: Vec::new(),
        consts: Vec::new(),
        enum_cases: Vec::new(),
        traits: Vec::new(),
    };
    for m in &c.members {
        match m {
            Member::Prop(p) => lo.prop(p),
            Member::Method(md) => lo.method(md),
            Member::Const(k) => lo.class_const(k),
            Member::EnumCase(e) => lo.enum_case(e),
            Member::TraitUse(t) => lo.trait_use(t),
        }
    }
    let MemberLower {
        props,
        methods,
        consts,
        enum_cases,
        traits,
        ..
    } = lo;

    let class = BcClass {
        name,
        name_bytes: interner.resolve(name).into(),
        parent,
        props,
        methods,
        line: mx.line(c.span.lo),
        parent_name,
        interfaces,
        kind,
        flags,
        consts,
        traits,
        enum_cases,
        enum_backing,
    };
    mx.sink.borrow_mut().classes[id as usize] = Some(class);
    Some(id)
}

/// `enum E: int` / `enum E: string`. The front end has already rejected any
/// other backing type (`Enum backing type must be int or string, X given`).
fn enum_backing_of(c: &ClassLike, diags: &mut Vec<Diagnostic>) -> EnumBackingType {
    let Some(b) = &c.backing else {
        return EnumBackingType::None;
    };
    match &b.kind {
        TypeKind::Builtin(Builtin::Int) => EnumBackingType::Int,
        TypeKind::Builtin(Builtin::String) => EnumBackingType::String,
        _ => {
            unsupported(diags, b.span, "enum backing type");
            EnumBackingType::None
        }
    }
}

/// The members of one class-like being lowered, and the sinks they land in.
struct MemberLower<'a, 'm> {
    mx: &'a ModuleCtx<'m>,
    diags: &'a mut Vec<Diagnostic>,
    /// The class being declared: its unit-local id and interned name. This is
    /// the lexical scope every method, hook and initializer thunk is compiled
    /// in, so `self::`, `static::` and `__CLASS__` resolve inside them.
    scope: (ClassId, IdentId),
    props: Vec<PropDef>,
    methods: Vec<BcMethod>,
    consts: Vec<ClassConstDef>,
    enum_cases: Vec<EnumCaseDef>,
    traits: Vec<BcTraitUse>,
}

impl<'m> MemberLower<'_, 'm> {
    /// The module's interner. The borrow is the module's, not `self`'s, so a
    /// name can be resolved while a member vector is being pushed to.
    fn interner(&self) -> &'m Interner {
        self.mx.interner
    }

    /// `[modifiers] [Type] $a = 1, $b;` and the hooked form
    /// `[modifiers] Type $a { get ...; set ...; }`.
    fn prop(&mut self, p: &PropMember) {
        let it = self.interner();
        let visibility = bc_vis(p.modifiers.vis.unwrap_or(AstVis::Public));
        let set_vis = p.modifiers.set_vis.map(bc_vis);
        let is_static = p.modifiers.static_;
        let readonly = p.modifiers.readonly;
        let ty = p.ty.as_ref().map(|t| lower_type_at(it, t));
        // A hooked declaration names exactly one property (the front end
        // rejects `$a, $b { ... }`), so one compile serves every item.
        let hooks = self.hooks(&p.hooks, p.items.first().map(|i| i.name), p.ty.as_ref());
        for item in &p.items {
            let (default, default_thunk) =
                self.initializer(item.default.as_ref(), item.span, ty.is_some());
            self.props.push(PropDef {
                name: it.resolve(item.name).into(),
                default,
                visibility,
                is_static,
                readonly,
                set_vis,
                ty: ty.clone(),
                hooks,
                default_thunk,
            });
        }
    }

    /// A method, and the properties its promoted parameters declare.
    fn method(&mut self, md: &MethodDecl) {
        let it = self.interner();
        // An abstract or interface method still gets a `Function`, with an
        // empty body, so its signature metadata (parameters, types) is there
        // for Reflection and for the signature-compatibility check.
        let body: &[Stmt] = md.body.as_deref().unwrap_or(&[]);
        let func = compile_function(
            self.mx,
            self.diags,
            FnSpec {
                name: md.name,
                params: &md.params,
                body,
                span: md.span,
                by_ref: md.by_ref,
                cur_class: Some(self.scope),
                is_static: md.modifiers.static_,
                ret: md.ret.as_ref(),
            },
        );
        self.methods.push(BcMethod {
            name_bytes: it.resolve(md.name).into(),
            func,
            visibility: bc_vis(md.modifiers.vis.unwrap_or(AstVis::Public)),
            is_static: md.modifiers.static_,
            is_abstract: md.modifiers.abstract_ || md.body.is_none(),
            is_final: md.modifiers.final_,
        });
        for param in &md.params {
            self.promoted_prop(param);
        }
    }

    /// `__construct(private readonly int $x)`: the parameter also declares a
    /// property of the class, at the constructor's position among the members
    /// (which is where php lays it out). It has no class-level default — php
    /// leaves a typed one uninitialized and an untyped one null — because the
    /// constructor assigns it on entry (`compile_params`).
    fn promoted_prop(&mut self, param: &Param) {
        let Some(m) = &param.promote else {
            return;
        };
        let it = self.interner();
        let ty = param.ty.as_ref().map(|t| lower_type_at(it, t));
        let hooks = self.hooks(&param.hooks, Some(param.name), param.ty.as_ref());
        self.props.push(PropDef {
            name: it.resolve(param.name).into(),
            default: if ty.is_some() {
                Value::Uninit
            } else {
                Value::Null
            },
            visibility: bc_vis(m.vis.unwrap_or(AstVis::Public)),
            is_static: false,
            readonly: m.readonly,
            set_vis: m.set_vis.map(bc_vis),
            ty,
            hooks,
            default_thunk: None,
        });
    }

    /// `[modifiers] const [Type] A = 1, B = 2;`.
    fn class_const(&mut self, k: &ConstMember) {
        let it = self.interner();
        let visibility = bc_vis(k.modifiers.vis.unwrap_or(AstVis::Public));
        let is_final = k.modifiers.final_;
        let ty = k.ty.as_ref().map(|t| lower_type_at(it, t));
        for item in &k.items {
            let (value, thunk) = match const_default(&item.value, it) {
                Some(v) => (Some(v), None),
                None => {
                    let scope = self.scope;
                    let t = compile_thunk_in(self.mx, self.diags, &item.value, Some(scope));
                    (None, Some(t))
                }
            };
            self.consts.push(ClassConstDef {
                name: it.resolve(item.name).into(),
                visibility,
                is_final,
                ty: ty.clone(),
                value,
                thunk,
            });
        }
    }

    /// `case NAME [= value];`.
    fn enum_case(&mut self, e: &EnumCase) {
        let it = self.interner();
        let value = match &e.value {
            None => None,
            Some(v) => match const_default(v, it) {
                Some(val) => Some(val),
                None => {
                    // `EnumCaseDef::value` is an eager `Value`: a case backed
                    // by a constant *expression* (`1 << 0`, `self::X`) has
                    // nowhere to put a thunk yet.
                    unsupported(self.diags, v.span(), "non-literal enum case value");
                    None
                }
            },
        };
        self.enum_cases.push(EnumCaseDef {
            name: it.resolve(e.name).into(),
            value,
        });
    }

    /// `use T1, T2 { T1::m insteadof T2; T2::m as private mine; }` — recorded
    /// verbatim; the runtime copies the members in at declaration time.
    fn trait_use(&mut self, t: &TraitUse) {
        let it = self.interner();
        let names = t
            .traits
            .iter()
            .map(|n| class_fqn(n, it).into_boxed_slice())
            .collect();
        let adaptations = t
            .adaptations
            .iter()
            .map(|a| match a {
                Adaptation::Precedence {
                    trait_,
                    method,
                    insteadof,
                    ..
                } => BcAdaptation::Precedence {
                    trait_: class_fqn(trait_, it).into_boxed_slice(),
                    method: it.resolve(*method).into(),
                    insteadof: insteadof
                        .iter()
                        .map(|n| class_fqn(n, it).into_boxed_slice())
                        .collect(),
                },
                Adaptation::Alias {
                    trait_,
                    method,
                    alias,
                    vis,
                    ..
                } => BcAdaptation::Alias {
                    trait_: trait_.as_ref().map(|n| class_fqn(n, it).into_boxed_slice()),
                    method: it.resolve(*method).into(),
                    vis: vis.map(bc_vis),
                    alias: alias.map(|a| it.resolve(a).into()),
                },
            })
            .collect();
        self.traits.push(BcTraitUse { names, adaptations });
    }

    /// Compile a property's hooks into functions of the class. `None` when the
    /// property has none; a hook declared abstract (`get;`, in an interface or
    /// on an `abstract` property) contributes no [`FuncId`] but still marks the
    /// property as hooked.
    fn hooks(&mut self, hooks: &[Hook], prop: Option<IdentId>, ty: Option<&Type>) -> Option<Hooks> {
        if hooks.is_empty() {
            return None;
        }
        let prop: Box<[u8]> = self.interner().resolve(prop?).into();
        let scope = self.scope;
        let mut out = Hooks {
            get: None,
            set: None,
        };
        for h in hooks {
            let f = compile_hook(self.mx, self.diags, &prop, ty, h, scope);
            match h.kind {
                HookKind::Get => out.get = f,
                HookKind::Set => out.set = f,
            }
        }
        Some(out)
    }

    /// A property default as the pair [`PropDef`] holds: the folded value when
    /// the initializer is a literal, otherwise a zero-argument thunk run in the
    /// class's scope on first use. A property with no initializer starts `null`
    /// when it is untyped and *uninitialized* when it has a declared type.
    fn initializer(
        &mut self,
        default: Option<&Expr>,
        span: Span,
        typed: bool,
    ) -> (Value, Option<FuncId>) {
        let Some(e) = default else {
            return (if typed { Value::Uninit } else { Value::Null }, None);
        };
        if let Some(v) = const_default(e, self.interner()) {
            return (v, None);
        }
        if !e.is_constant_shape() {
            self.diags.push(
                Diagnostic::error(
                    NON_CONST_PROP_DEFAULT,
                    "property default must be a constant expression",
                )
                .with_primary(span, "not a constant"),
            );
            return (Value::Null, None);
        }
        let scope = self.scope;
        let t = compile_thunk_in(self.mx, self.diags, e, Some(scope));
        (Value::Null, Some(t))
    }
}

/// The FQN bytes of a class-position name: the resolver's `fqn` when the
/// HIR pass filled it, else the spelling.
pub(crate) fn class_fqn(name: &rphp_ast::v2::Name, interner: &Interner) -> Vec<u8> {
    let id = match name.resolved {
        Some(rphp_ast::v2::Resolved::Class { fqn, .. }) => fqn,
        _ => name.text,
    };
    interner.resolve(id).to_vec()
}

/// Constant-fold a literal expression: scalars, a unary minus over a numeric
/// literal, and array literals of foldable items. Anything else returns `None`
/// and its caller compiles a thunk instead.
pub(crate) fn const_default(e: &Expr, interner: &Interner) -> Option<Value> {
    Some(match e {
        Expr::Null(_) => Value::Null,
        Expr::Bool(b, _) => Value::Bool(*b),
        Expr::Int(i, _) => Value::Int(*i),
        Expr::Float(f, _) => Value::Float(*f),
        Expr::Str(id, _) => Value::Str(Str::new(interner.resolve(*id))),
        Expr::Unary {
            op: UnOp::Neg,
            expr,
            ..
        } => match const_default(expr, interner)? {
            Value::Int(i) => i
                .checked_neg()
                .map(Value::Int)
                .unwrap_or(Value::Float(-(i as f64))),
            Value::Float(f) => Value::Float(-f),
            _ => return None,
        },
        Expr::Array { items, syntax, .. } if !syntax.is_list() => {
            let mut arr = Array::new();
            for item in items {
                let ArrayItem {
                    key,
                    value: Some(value),
                    by_ref: false,
                    spread: false,
                    ..
                } = item
                else {
                    return None;
                };
                let v = const_default(value, interner)?;
                match key {
                    Some(k) => {
                        let k = array_key(&const_default(k, interner)?)?;
                        arr.set(k, v);
                    }
                    None => arr.push(v),
                }
            }
            Value::Array(arr)
        }
        _ => return None,
    })
}

/// Map an AST visibility to its bytecode counterpart.
pub(crate) fn bc_vis(v: AstVis) -> Visibility {
    match v {
        AstVis::Public => Visibility::Public,
        AstVis::Protected => Visibility::Protected,
        AstVis::Private => Visibility::Private,
    }
}
