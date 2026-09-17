//! Class lowering: the declaration pre-pass (every class-like in the unit
//! gets a [`ClassId`] up front so `extends` can name a class declared
//! later or conditionally) and the `rphp_bytecode::Class` assembly.
//!
//! The lowered slice is `class C [extends P] { [vis] $p = <const>; [vis]
//! function m(...) { ... } }`: single inheritance, untyped (types are
//! ignored as metadata) properties with constant-foldable defaults, and
//! non-static, non-abstract methods. Interfaces, traits, enums, class
//! constants, static members, readonly/abstract/hook features and promotion
//! are reported as `RPHP_E0300` (plan E6).

use std::collections::HashMap;

use rphp_ast::v2::visit::{walk_expr, walk_stmt, Visitor};
use rphp_ast::v2::{
    ArrayItem, ClassKind, ClassLike, Expr, Member, Modifiers, Program, Stmt, UnOp,
    Visibility as AstVis,
};
use rphp_bytecode::{Class as BcClass, ClassId, Method as BcMethod, PropDef, Visibility};
use rphp_diagnostics::Diagnostic;
use rphp_intern::{IdentId, Interner};
use rphp_span::Span;
use rphp_value::{array_key, Array, Str, Value};

use crate::func::{compile_function, FnSpec, ModuleCtx};
use crate::{name_is_global, unsupported, NON_CONST_PROP_DEFAULT, REDECLARED_CLASS, UNDEFINED_CLASS};

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
/// pre-assigned id (methods go to the function sink). `None` when the
/// declaration kind itself is not lowered.
pub(crate) fn compile_class(
    mx: &ModuleCtx<'_>,
    diags: &mut Vec<Diagnostic>,
    c: &ClassLike,
) -> Option<ClassId> {
    let interner = mx.interner;
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
    let id = *mx.class_ids.get(&(c as *const ClassLike)).expect("class numbered by the pre-pass");
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
            if name_is_global(p) {
                let key = interner.resolve(p.text).to_ascii_lowercase();
                match mx.class_map.get(key.as_slice()) {
                    Some(&pid) => Some(pid),
                    None => {
                        diags.push(
                            Diagnostic::error(
                                UNDEFINED_CLASS,
                                format!("class \"{}\" not found", interner.resolve_lossy(p.text)),
                            )
                            .with_primary(p.span, "unknown parent class"),
                        );
                        None
                    }
                }
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
                    let default = match &item.default {
                        None => Value::Null,
                        Some(e) => prop_default(e, item.span, interner, diags),
                    };
                    props.push(PropDef {
                        name: interner.resolve(item.name).into(),
                        default,
                        visibility,
                    });
                }
            }
            Member::Method(md) => {
                if md.modifiers.readonly || md.modifiers.set_vis.is_some() {
                    check_member_modifiers(&md.modifiers, diags, "method");
                }
                if md.modifiers.static_ {
                    unsupported(diags, md.modifiers.span, "static method");
                }
                if md.modifiers.abstract_ {
                    unsupported(diags, md.modifiers.span, "abstract method");
                }
                let Some(body) = &md.body else {
                    if !md.modifiers.abstract_ {
                        unsupported(diags, md.span, "method without a body");
                    }
                    continue;
                };
                let visibility = bc_vis(md.modifiers.vis.unwrap_or(AstVis::Public));
                let func = compile_function(
                    mx,
                    diags,
                    FnSpec {
                        name: md.name,
                        params: &md.params,
                        body,
                        span: md.span,
                        by_ref: md.by_ref,
                        cur_class: Some((id, name)),
                        is_static: md.modifiers.static_,
                    },
                );
                methods.push(BcMethod {
                    name_bytes: interner.resolve(md.name).into(),
                    func,
                    visibility,
                });
            }
            Member::Const(k) => unsupported(diags, k.span, "class constant"),
            Member::EnumCase(e) => unsupported(diags, e.span, "enum case"),
            Member::TraitUse(t) => unsupported(diags, t.span, "trait use"),
        }
    }
    let class = BcClass {
        name,
        name_bytes: interner.resolve(name).into(),
        parent,
        props,
        methods,
        line: mx.line(c.span.lo),
    };
    mx.sink.borrow_mut().classes[id as usize] = Some(class);
    Some(id)
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

/// A property default as a value: a literal, a unary minus over a numeric
/// literal, or an array of such. Anything that is not constant-shaped is
/// `RPHP_E0108`; a constant expression the slice does not fold yet
/// (constants, class constants, operators) is `RPHP_E0300`. Either way the
/// property gets `null` so compilation goes on.
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

/// Constant-fold a literal expression: scalars, a unary minus over a numeric
/// literal, and array literals of foldable items. Anything else returns `None`.
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

// `IdentId` is part of the sink's class naming; keep the import used.
#[allow(dead_code)]
fn _ident(_: IdentId) {}
