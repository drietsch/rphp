//! Class lowering: the declaration pre-pass, the compile-time class tables
//! and the final `rphp_bytecode::Class` assembly.
//!
//! The lowered slice is `class C [extends P] { [vis] $p = <const>; [vis]
//! function m(...) { ... } }`: single inheritance, untyped (types are
//! ignored as metadata) properties with literal defaults, and non-static,
//! non-abstract methods. Interfaces, traits, enums, class constants, static
//! members, readonly/abstract/hook features and promotion are reported as
//! `RPHP_E0300`.

use std::collections::HashMap;

use rphp_ast::v2::{
    ClassKind, ClassLike, Expr, Member, MethodDecl, Modifiers, NameKind, PropItem, UnOp,
    Visibility as AstVis,
};
use rphp_bytecode::{Class as BcClass, ClassId, FuncId, Method as BcMethod, PropDef, Visibility};
use rphp_diagnostics::Diagnostic;
use rphp_intern::{IdentId, Interner};
use rphp_span::Span;
use rphp_value::{Str, Value};

use crate::{unsupported, NON_CONST_PROP_DEFAULT, REDECLARED_CLASS, UNDEFINED_CLASS};

/// A class's own methods as (name, compiled id) pairs.
pub(crate) type MethodTable = Vec<(Box<[u8]>, FuncId)>;

/// Shared class lookup tables threaded into every function compilation.
/// Indexed by [`ClassId`] except `map` (name -> id): the resolved parent of
/// each class, whether each has a constructor anywhere in its chain, and each
/// class's *own* methods (name -> FuncId) for compile-time scoped-call
/// resolution.
pub(crate) struct ClassCtx<'a> {
    pub(crate) map: &'a HashMap<IdentId, ClassId>,
    pub(crate) has_ctor: &'a [bool],
    pub(crate) parent: &'a [Option<ClassId>],
    pub(crate) methods: &'a [MethodTable],
}

impl ClassCtx<'_> {
    /// Resolve a method name on `class`, walking up the chain (compile time).
    pub(crate) fn resolve_method(&self, class: ClassId, name: &[u8]) -> Option<FuncId> {
        let mut cur = Some(class);
        while let Some(cid) = cur {
            for (n, f) in &self.methods[cid as usize] {
                if n.eq_ignore_ascii_case(name) {
                    return Some(*f);
                }
            }
            cur = self.parent[cid as usize];
        }
        None
    }
}

/// A declared class as the driver sees it after the shape checks: its
/// lowered properties and methods, in declaration order.
pub(crate) struct ClassInfo<'a> {
    pub(crate) name: IdentId,
    pub(crate) span: Span,
    /// The `extends` name, if any (looked up after all classes are known).
    pub(crate) parent: Option<IdentId>,
    pub(crate) props: Vec<PropInfo<'a>>,
    pub(crate) methods: Vec<MethodInfo<'a>>,
}

pub(crate) struct PropInfo<'a> {
    pub(crate) item: &'a PropItem,
    pub(crate) visibility: Visibility,
}

pub(crate) struct MethodInfo<'a> {
    pub(crate) decl: &'a MethodDecl,
    /// The body (never `None`: abstract methods are rejected in the pre-pass).
    pub(crate) body: &'a [rphp_ast::v2::Stmt],
    pub(crate) visibility: Visibility,
}

/// The class pre-pass over the hoisted top-level declarations: assign
/// [`ClassId`]s in declaration order, reserve a [`FuncId`] per method starting
/// at `first_method_id`, and check every declaration against the lowered
/// slice. Returns the classes, their method ids (parallel to `.methods`) and
/// the name -> id map.
pub(crate) fn collect_classes<'a>(
    decls: &[&'a ClassLike],
    interner: &Interner,
    first_method_id: FuncId,
    diags: &mut Vec<Diagnostic>,
) -> (
    Vec<ClassInfo<'a>>,
    Vec<Vec<FuncId>>,
    HashMap<IdentId, ClassId>,
) {
    let mut class_map: HashMap<IdentId, ClassId> = HashMap::new();
    let mut classes: Vec<ClassInfo<'a>> = Vec::new();
    let mut method_ids: Vec<Vec<FuncId>> = Vec::new();
    let mut next_method_id = first_method_id;
    for c in decls {
        let Some(info) = check_class(c, interner, diags) else {
            continue;
        };
        if class_map.contains_key(&info.name) {
            diags.push(
                Diagnostic::error(
                    REDECLARED_CLASS,
                    format!(
                        "cannot redeclare class {}",
                        interner.resolve_lossy(info.name)
                    ),
                )
                .with_primary(info.span, "duplicate declaration"),
            );
            continue;
        }
        class_map.insert(info.name, classes.len() as ClassId);
        let ids: Vec<FuncId> = info
            .methods
            .iter()
            .map(|_| {
                let id = next_method_id;
                next_method_id += 1;
                id
            })
            .collect();
        method_ids.push(ids);
        classes.push(info);
    }
    (classes, method_ids, class_map)
}

/// Check one class-like declaration against the slice and extract its
/// lowered members. `None` when the declaration kind itself is not lowered
/// (interfaces, traits, enums, anonymous classes); member-level rejections
/// are reported but keep the class so later references still resolve.
fn check_class<'a>(
    c: &'a ClassLike,
    interner: &Interner,
    diags: &mut Vec<Diagnostic>,
) -> Option<ClassInfo<'a>> {
    match c.kind {
        ClassKind::Class => {}
        ClassKind::Interface => {
            unsupported(diags, c.span, "interface declaration");
            return None;
        }
        ClassKind::Trait => {
            unsupported(diags, c.span, "trait declaration");
            return None;
        }
        ClassKind::Enum => {
            unsupported(diags, c.span, "enum declaration");
            return None;
        }
    }
    let Some(name) = c.name else {
        unsupported(diags, c.span, "anonymous class");
        return None;
    };
    if c.modifiers.abstract_ {
        unsupported(diags, c.modifiers.span, "abstract class");
    }
    if c.modifiers.readonly {
        unsupported(diags, c.modifiers.span, "readonly class");
    }
    // `final` has no effect the slice observes (no declaration-time checks).
    let parent = match c.extends.as_slice() {
        [] => None,
        [p] => {
            if matches!(p.kind, NameKind::Unqualified | NameKind::FullyQualified) {
                Some(p.text)
            } else {
                unsupported(diags, p.span, "namespaced parent class name");
                None
            }
        }
        [_, second, ..] => {
            unsupported(diags, second.span, "multiple parents on a class");
            None
        }
    };
    for i in &c.implements {
        unsupported(diags, i.span, "interface implementation");
    }
    if let Some(b) = &c.backing {
        unsupported(diags, b.span, "enum backing type");
    }
    let mut props = Vec::new();
    let mut methods = Vec::new();
    for m in &c.members {
        match m {
            Member::Prop(p) => {
                check_member_modifiers(&p.modifiers, diags, "property");
                if p.modifiers.abstract_ || p.modifiers.final_ {
                    unsupported(diags, p.modifiers.span, "abstract/final property");
                }
                if !p.hooks.is_empty() {
                    unsupported(diags, p.span, "property hooks");
                }
                let visibility = bc_vis(p.modifiers.vis.unwrap_or(AstVis::Public));
                for item in &p.items {
                    props.push(PropInfo { item, visibility });
                }
            }
            Member::Method(md) => {
                check_member_modifiers(&md.modifiers, diags, "method");
                if md.modifiers.abstract_ {
                    unsupported(diags, md.modifiers.span, "abstract method");
                }
                if md.by_ref {
                    unsupported(diags, md.span, "method returning by reference");
                }
                let Some(body) = &md.body else {
                    if !md.modifiers.abstract_ {
                        unsupported(diags, md.span, "method without a body");
                    }
                    continue;
                };
                let visibility = bc_vis(md.modifiers.vis.unwrap_or(AstVis::Public));
                methods.push(MethodInfo {
                    decl: md,
                    body,
                    visibility,
                });
            }
            Member::Const(k) => unsupported(diags, k.span, "class constant"),
            Member::EnumCase(e) => unsupported(diags, e.span, "enum case"),
            Member::TraitUse(t) => unsupported(diags, t.span, "trait use"),
        }
    }
    let _ = interner;
    Some(ClassInfo {
        name,
        span: c.span,
        parent,
        props,
        methods,
    })
}

/// Reject the member modifiers the slice does not model (`static`,
/// `readonly`, asymmetric visibility).
fn check_member_modifiers(m: &Modifiers, diags: &mut Vec<Diagnostic>, what: &str) {
    if m.static_ {
        unsupported(diags, m.span, &format!("static {what}"));
    }
    if m.readonly {
        unsupported(diags, m.span, &format!("readonly {what}"));
    }
    if m.set_vis.is_some() {
        unsupported(diags, m.span, "asymmetric visibility");
    }
}

/// Resolve every `extends` to a [`ClassId`] and diagnose unknown parents and
/// inheritance cycles (runtime chain walks must terminate).
pub(crate) fn resolve_parents(
    classes: &[ClassInfo<'_>],
    class_map: &HashMap<IdentId, ClassId>,
    interner: &Interner,
    diags: &mut Vec<Diagnostic>,
) -> Vec<Option<ClassId>> {
    let n_classes = classes.len();
    let parent_id: Vec<Option<ClassId>> = classes
        .iter()
        .map(|c| match c.parent {
            None => None,
            Some(pname) => {
                let p = class_map.get(&pname).copied();
                if p.is_none() {
                    diags.push(
                        Diagnostic::error(
                            UNDEFINED_CLASS,
                            format!("class \"{}\" not found", interner.resolve_lossy(pname)),
                        )
                        .with_primary(c.span, "unknown parent class"),
                    );
                }
                p
            }
        })
        .collect();
    for (ci, c) in classes.iter().enumerate() {
        let mut cur = parent_id[ci];
        let mut hops = 0usize;
        while let Some(p) = cur {
            hops += 1;
            if hops > n_classes {
                diags.push(
                    Diagnostic::error(
                        REDECLARED_CLASS,
                        format!(
                            "class \"{}\" has a cyclic inheritance chain",
                            interner.resolve_lossy(c.name)
                        ),
                    )
                    .with_primary(c.span, "cyclic `extends`"),
                );
                break;
            }
            cur = parent_id[p as usize];
        }
    }
    parent_id
}

/// Whether each class has a `__construct` anywhere up its chain (a `new`
/// needs a constructor call iff one exists).
pub(crate) fn constructor_chain(
    classes: &[ClassInfo<'_>],
    parent_id: &[Option<ClassId>],
    interner: &Interner,
) -> Vec<bool> {
    let n_classes = classes.len();
    let own_ctor: Vec<bool> = classes
        .iter()
        .map(|c| {
            c.methods.iter().any(|m| {
                interner
                    .resolve(m.decl.name)
                    .eq_ignore_ascii_case(b"__construct")
            })
        })
        .collect();
    (0..n_classes)
        .map(|ci| {
            let mut cur = Some(ci as ClassId);
            let mut hops = 0usize;
            while let Some(cid) = cur {
                if own_ctor[cid as usize] {
                    return true;
                }
                hops += 1;
                if hops > n_classes {
                    break; // cycle (already diagnosed)
                }
                cur = parent_id[cid as usize];
            }
            false
        })
        .collect()
}

/// Per-class own-method tables (name -> FuncId) for compile-time `self::` /
/// `parent::` / `Class::` resolution.
pub(crate) fn method_tables(
    classes: &[ClassInfo<'_>],
    method_ids: &[Vec<FuncId>],
    interner: &Interner,
) -> Vec<MethodTable> {
    classes
        .iter()
        .enumerate()
        .map(|(ci, c)| {
            c.methods
                .iter()
                .enumerate()
                .map(|(mi, m)| (interner.resolve(m.decl.name).into(), method_ids[ci][mi]))
                .collect()
        })
        .collect()
}

/// Build the bytecode classes: defaults evaluated, methods linked to ids.
pub(crate) fn lower_classes(
    classes: &[ClassInfo<'_>],
    method_ids: &[Vec<FuncId>],
    parent_id: &[Option<ClassId>],
    interner: &Interner,
    diags: &mut Vec<Diagnostic>,
) -> Vec<BcClass> {
    classes
        .iter()
        .enumerate()
        .map(|(ci, c)| {
            let props = c
                .props
                .iter()
                .map(|p| {
                    let default = match &p.item.default {
                        None => Value::Null,
                        Some(e) => prop_default(e, p.item.span, interner, diags),
                    };
                    PropDef {
                        name: interner.resolve(p.item.name).into(),
                        default,
                        visibility: p.visibility,
                    }
                })
                .collect();
            let methods = c
                .methods
                .iter()
                .enumerate()
                .map(|(mi, m)| BcMethod {
                    name_bytes: interner.resolve(m.decl.name).into(),
                    func: method_ids[ci][mi],
                    visibility: m.visibility,
                })
                .collect();
            BcClass {
                name: c.name,
                name_bytes: interner.resolve(c.name).into(),
                parent: parent_id[ci],
                props,
                methods,
            }
        })
        .collect()
}

/// A property default as a value: a literal or a unary minus over a numeric
/// literal. Anything that is not constant-shaped is `RPHP_E0108`; a constant
/// expression the slice does not fold yet (arrays, constants, operators) is
/// `RPHP_E0300`. Either way the property gets `null` so compilation goes on.
fn prop_default(e: &Expr, span: Span, interner: &Interner, diags: &mut Vec<Diagnostic>) -> Value {
    if let Some(v) = const_default(e, interner) {
        return v;
    }
    if e.is_constant_shape() {
        unsupported(diags, e.span(), "non-literal property default");
    } else {
        diags.push(
            Diagnostic::error(
                NON_CONST_PROP_DEFAULT,
                "property default must be a constant expression",
            )
            .with_primary(span, "not a constant"),
        );
    }
    Value::Null
}

/// Constant-fold a property default. Only literals (and a unary minus over a
/// numeric literal) are supported; anything else returns `None`.
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
