//! Functions, closures, arrow functions, parameters and property hooks.

use mago_span::HasSpan;
use mago_syntax::cst::{
    ArrowFunction, Closure as MagoClosure, Function, FunctionLikeParameter,
    FunctionLikeParameterList, Modifier, PropertyHook, PropertyHookBody, PropertyHookConcreteBody,
    PropertyHookList,
};
use rphp_ast::v2::{
    ArrowFn, Builtin, Closure, ClosureUse, Expr, FuncDecl, Hook, HookBody, HookKind, Modifiers,
    Param, Type,
};

use super::conformance::{check_const_expr, ConstCtx};
use super::ctx::{eq_ci, Ctx, FnKind, FnScope, RetKind, ReturnValue};
use super::types::{self, TypePos};
use super::{attrs, class, docs, expr, names, stmt};

/// Who owns a parameter list; decides the promotion rules.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ParamOwner<'s> {
    /// A named function, closure or arrow function.
    Function,
    /// A method; `is_ctor` for `__construct`, `is_abstract` when it has no
    /// body (abstract or interface method).
    Method {
        /// The method is `__construct`.
        is_ctor: bool,
        /// The method is abstract or declared in an interface.
        is_abstract: bool,
        /// The owning class name for messages.
        class: &'s str,
    },
    /// A `set` hook parameter list.
    Hook,
}

/// The return-type kind for the function scope.
pub(crate) fn ret_kind(ret: Option<&Type>) -> RetKind {
    match ret {
        None => RetKind::None,
        Some(t) if types::is_builtin(t, Builtin::Void) => RetKind::Void,
        Some(t) if types::is_builtin(t, Builtin::Never) => RetKind::Never,
        Some(t) => RetKind::Typed {
            nullable: type_accepts_null(t),
        },
    }
}

/// Apply the `return` rules once a body has been converted.
pub(crate) fn finish_fn(ctx: &mut Ctx<'_, '_>, scope: FnScope) {
    for (span, value) in scope.returns {
        match (scope.ret, value) {
            (RetKind::Void, ReturnValue::Null) => ctx.reject(
                "A void function must not return a value (did you mean \"return;\" instead of \"return null;\"?)",
                span,
            ),
            (RetKind::Void, ReturnValue::Value) => {
                ctx.reject("A void function must not return a value", span);
            }
            (RetKind::Never, _) => ctx.reject("A never-returning function must not return", span),
            (RetKind::Typed { nullable }, ReturnValue::None) if !scope.has_yield => {
                if nullable {
                    ctx.reject(
                        "A function with return type must return a value (did you mean \"return null;\" instead of \"return;\"?)",
                        span,
                    );
                } else {
                    ctx.reject("A function with return type must return a value", span);
                }
            }
            _ => {}
        }
    }
}

/// A named function declaration.
pub(crate) fn function(ctx: &mut Ctx<'_, '_>, f: &Function<'_>) -> FuncDecl {
    let span = ctx.sp(f.span());
    let doc = docs::decl_doc(ctx, &f.attribute_lists, f.function.span.start.offset);
    let attrs = attrs::groups(ctx, &f.attribute_lists);
    if !eq_ci(f.name.value, "readonly") {
        names::check_reserved(ctx, &f.name, "(");
    }
    let name = names::local(ctx, &f.name);
    let display = ctx.interner.resolve_lossy(name).into_owned();
    let ret = f.return_type_hint.as_ref().map(|r| types::hint(ctx, &r.hint));
    ctx.push_fn(FnKind::Function, ret_kind(ret.as_ref()), Some(display));
    ctx.fn_scope().by_ref = f.ampersand.is_some();
    if let Some(t) = &ret {
        types::check(ctx, t, TypePos::Return, "");
    }
    let params = params(ctx, &f.parameter_list, ParamOwner::Function);
    let body = stmt::block_stmts(ctx, &f.body);
    let scope = ctx.pop_fn();
    finish_fn(ctx, scope);
    FuncDecl {
        attrs,
        name,
        by_ref: f.ampersand.is_some(),
        params,
        ret,
        body,
        doc,
        span,
    }
}

/// An anonymous function.
pub(crate) fn closure(ctx: &mut Ctx<'_, '_>, c: &MagoClosure<'_>) -> Expr {
    let span = ctx.sp(c.span());
    let after_attrs = c
        .r#static
        .as_ref()
        .map_or(c.function.span.start.offset, |s| s.span.start.offset);
    let doc = docs::decl_doc(ctx, &c.attribute_lists, after_attrs);
    let attrs = attrs::groups(ctx, &c.attribute_lists);
    let ret = c.return_type_hint.as_ref().map(|r| types::hint(ctx, &r.hint));
    ctx.push_fn(FnKind::Closure, ret_kind(ret.as_ref()), Some("{closure}".into()));
    ctx.fn_scope().by_ref = c.ampersand.is_some();
    if let Some(t) = &ret {
        types::check(ctx, t, TypePos::Return, "");
    }
    let params = params(ctx, &c.parameter_list, ParamOwner::Function);
    let mut uses = Vec::new();
    if let Some(clause) = &c.use_clause {
        for v in clause.variables.iter() {
            let name = ctx.intern_var(v.variable.name);
            let use_span = ctx.sp(v.span());
            if Some(name) == ctx.sv.this {
                ctx.reject("Cannot use $this as lexical variable", use_span);
            } else if uses.iter().any(|u: &ClosureUse| u.name == name) {
                let text = ctx.interner.resolve_lossy(name).into_owned();
                ctx.reject(format!("Cannot use variable ${text} twice"), use_span);
            } else if params.iter().any(|p| p.name == name) {
                let text = ctx.interner.resolve_lossy(name).into_owned();
                ctx.reject(
                    format!("Cannot use lexical variable ${text} as a parameter name"),
                    use_span,
                );
            }
            uses.push(ClosureUse {
                name,
                by_ref: v.ampersand.is_some(),
                span: use_span,
            });
        }
    }
    let body = stmt::block_stmts(ctx, &c.body);
    let scope = ctx.pop_fn();
    finish_fn(ctx, scope);
    Expr::Closure(Box::new(Closure {
        static_: c.r#static.is_some(),
        by_ref: c.ampersand.is_some(),
        attrs,
        params,
        uses,
        ret,
        body,
        doc,
        span,
    }))
}

/// An arrow function.
pub(crate) fn arrow_fn(ctx: &mut Ctx<'_, '_>, f: &ArrowFunction<'_>) -> Expr {
    let span = ctx.sp(f.span());
    let after_attrs = f
        .r#static
        .as_ref()
        .map_or(f.r#fn.span.start.offset, |s| s.span.start.offset);
    let doc = docs::decl_doc(ctx, &f.attribute_lists, after_attrs);
    let attrs = attrs::groups(ctx, &f.attribute_lists);
    let ret = f.return_type_hint.as_ref().map(|r| types::hint(ctx, &r.hint));
    let kind = ret_kind(ret.as_ref());
    ctx.push_fn(FnKind::ArrowFn, kind, Some("{closure}".into()));
    if let Some(t) = &ret {
        types::check(ctx, t, TypePos::Return, "");
    }
    let params = params(ctx, &f.parameter_list, ParamOwner::Function);
    let body = expr::expr(ctx, f.expression);
    let scope = ctx.pop_fn();
    // `fn(): never => expr` compiles the body as a statement, not a return.
    if kind == RetKind::Void {
        ctx.reject("A void function must not return a value", body.span());
    }
    let _ = scope;
    Expr::ArrowFn(Box::new(ArrowFn {
        static_: f.r#static.is_some(),
        by_ref: f.ampersand.is_some(),
        attrs,
        params,
        ret,
        body,
        doc,
        span,
    }))
}

/// A parameter list with PHP's parameter rules applied.
pub(crate) fn params(
    ctx: &mut Ctx<'_, '_>,
    list: &FunctionLikeParameterList<'_>,
    owner: ParamOwner<'_>,
) -> Vec<Param> {
    let n = list.parameters.len();
    let mut out: Vec<Param> = Vec::with_capacity(n);
    for (i, p) in list.parameters.iter().enumerate() {
        let prm = param(ctx, p, owner);
        if prm.variadic && i + 1 != n {
            ctx.reject("Only the last parameter can be variadic", prm.span);
        }
        if out.iter().any(|q| q.name == prm.name) {
            let text = ctx.interner.resolve_lossy(prm.name).into_owned();
            ctx.reject(format!("Redefinition of parameter ${text}"), prm.span);
        }
        out.push(prm);
    }
    out
}

fn param(ctx: &mut Ctx<'_, '_>, p: &FunctionLikeParameter<'_>, owner: ParamOwner<'_>) -> Param {
    let span = ctx.sp(p.span());
    let attrs = attrs::groups(ctx, &p.attribute_lists);
    let name = ctx.intern_var(p.variable.name);
    let name_span = ctx.sp(p.variable.span);
    if Some(name) == ctx.sv.this {
        ctx.reject("Cannot use $this as parameter", name_span);
    }
    let mut promote = promotion_modifiers(ctx, &p.modifiers);
    if promote.is_none() && p.hooks.is_some() {
        // A hooked parameter is a promoted property even without modifiers.
        promote = Some(Modifiers::default());
    }
    let ty = p.hint.as_ref().map(|h| types::hint(ctx, h));
    let variadic = p.ellipsis.is_some();
    let prop_owner = match owner {
        ParamOwner::Method { class, .. } => {
            format!("{class}::${}", ctx.interner.resolve_lossy(name))
        }
        _ => format!("${}", ctx.interner.resolve_lossy(name)),
    };
    if let Some(t) = &ty {
        types::check(ctx, t, TypePos::Param, &prop_owner);
        if promote.is_some() {
            types::check(ctx, t, TypePos::Prop, &prop_owner);
        }
    }
    let default = p.default_value.as_ref().map(|d| expr::const_expr(ctx, d.value));
    if let Some(d) = &default {
        check_const_expr(ctx, d, ConstCtx::ParamDefault);
        if variadic {
            ctx.reject("Variadic parameter cannot have a default value", d.span());
        }
    }
    if let Some(m) = &promote {
        match owner {
            ParamOwner::Method { is_ctor: true, is_abstract: false, .. } => {}
            ParamOwner::Method { is_ctor: true, is_abstract: true, .. } => {
                ctx.reject("Cannot declare promoted property in an abstract constructor", span);
            }
            _ => ctx.reject("Cannot declare promoted property outside a constructor", span),
        }
        if variadic {
            ctx.reject("Cannot declare variadic promoted property", span);
        }
        if (m.readonly || class::in_readonly_class(ctx)) && ty.is_none() {
            ctx.reject(format!("Readonly property {prop_owner} must have type"), span);
        }
        if (m.readonly || class::in_readonly_class(ctx)) && p.hooks.is_some() {
            ctx.reject("Hooked properties cannot be readonly", span);
        }
        if m.set_vis.is_some() && ty.is_none() {
            ctx.reject(
                format!("Property with asymmetric visibility {prop_owner} must have type"),
                span,
            );
        }
        if let (Some(t), Some(Expr::Null(_))) = (&ty, &default) {
            if !type_accepts_null(t) {
                ctx.reject(
                    format!(
                        "Cannot use null as default value for parameter ${} of type {}",
                        ctx.interner.resolve_lossy(name),
                        types::display(ctx, t)
                    ),
                    span,
                );
            }
        }
    }
    let hooks = match &p.hooks {
        Some(list) => {
            let private = promote.is_some_and(|m| m.vis == Some(rphp_ast::v2::Visibility::Private));
            hooks(ctx, list, &prop_owner, false, false, private)
        }
        None => Vec::new(),
    };
    Param {
        attrs,
        name,
        ty,
        default,
        by_ref: p.ampersand.is_some(),
        variadic,
        promote,
        hooks,
        span,
    }
}

/// Whether a declared type admits `null` syntactically.
pub(crate) fn type_accepts_null(t: &Type) -> bool {
    use rphp_ast::v2::TypeKind;
    match &t.kind {
        TypeKind::Nullable(_) => true,
        TypeKind::Builtin(Builtin::Null | Builtin::Mixed) => true,
        TypeKind::Union(ms) => ms.iter().any(type_accepts_null),
        _ => false,
    }
}

/// The promotion modifiers of a parameter (`None` when there are none).
fn promotion_modifiers(ctx: &mut Ctx<'_, '_>, mods: &mago_syntax::cst::Sequence<'_, Modifier<'_>>) -> Option<Modifiers> {
    if mods.is_empty() {
        return None;
    }
    for m in mods.iter() {
        let s = ctx.sp(m.span());
        match m {
            Modifier::Static(_) => ctx.reject("Cannot use the static modifier on a parameter", s),
            Modifier::Abstract(_) => {
                ctx.reject("Cannot use the abstract modifier on a parameter", s)
            }
            _ => {}
        }
    }
    Some(class::modifiers(ctx, mods, class::ModTarget::Param))
}

/// Property hooks. `abstract_ok` allows body-less hooks (abstract property
/// or interface); `in_interface` forbids bodies.
pub(crate) fn hooks(
    ctx: &mut Ctx<'_, '_>,
    list: &PropertyHookList<'_>,
    owner: &str,
    abstract_ok: bool,
    in_interface: bool,
    owner_private: bool,
) -> Vec<Hook> {
    let mut out: Vec<Hook> = Vec::new();
    for h in list.hooks.iter() {
        let hook = hook(ctx, h, owner, abstract_ok, in_interface, owner_private);
        if out.iter().any(|x| x.kind == hook.kind) {
            ctx.reject(
                format!("Cannot redeclare property hook \"{}\"", hook.kind.as_str()),
                hook.span,
            );
        }
        out.push(hook);
    }
    out
}

fn hook(
    ctx: &mut Ctx<'_, '_>,
    h: &PropertyHook<'_>,
    owner: &str,
    abstract_ok: bool,
    in_interface: bool,
    owner_private: bool,
) -> Hook {
    let span = ctx.sp(h.span());
    let attrs = attrs::groups(ctx, &h.attribute_lists);
    let mut final_ = false;
    for m in h.modifiers.iter() {
        let s = ctx.sp(m.span());
        match m {
            Modifier::Final(_) => final_ = true,
            other => {
                let word = modifier_word(other);
                ctx.reject(format!("Cannot use the {word} modifier on a property hook"), s);
            }
        }
    }
    if final_ && owner_private {
        ctx.reject("Property hook cannot be both final and private", span);
    }
    let kind = if eq_ci(h.name.value, "get") {
        HookKind::Get
    } else if eq_ci(h.name.value, "set") {
        HookKind::Set
    } else {
        let bad = String::from_utf8_lossy(h.name.value).into_owned();
        ctx.reject(
            format!("Unknown hook \"{bad}\" for property {owner}, expected \"get\" or \"set\""),
            ctx.sp(h.name.span),
        );
        HookKind::Get
    };
    let params = h.parameter_list.as_ref().map(|list| {
        let ps = params(ctx, list, ParamOwner::Hook);
        match kind {
            HookKind::Get => ctx.reject(
                format!("get hook of property {owner} must not have a parameter list"),
                ctx.sp(list.span()),
            ),
            HookKind::Set if ps.len() != 1 => ctx.reject(
                format!("set hook of property {owner} must accept exactly one parameters"),
                ctx.sp(list.span()),
            ),
            HookKind::Set => {}
        }
        ps
    });
    ctx.push_fn(FnKind::Hook, RetKind::None, Some(format!("{owner}::{}", kind.as_str())));
    let prop_name = owner.rsplit("::$").next().unwrap_or("").as_bytes().to_vec();
    ctx.fn_scope().hook = Some((prop_name, kind));
    let body = match &h.body {
        PropertyHookBody::Abstract(_) => {
            if !abstract_ok {
                ctx.reject("Non-abstract property hook must have a body", span);
            }
            HookBody::Abstract
        }
        PropertyHookBody::Concrete(c) => {
            if in_interface {
                ctx.reject("Abstract property hook cannot have body", span);
            }
            match c {
                PropertyHookConcreteBody::Block(b) => HookBody::Block(stmt::block_stmts(ctx, b)),
                PropertyHookConcreteBody::Expression(e) => {
                    HookBody::Expr(expr::expr(ctx, e.expression))
                }
            }
        }
    };
    let _ = ctx.pop_fn();
    Hook {
        kind,
        attrs,
        final_,
        by_ref: h.ampersand.is_some(),
        params,
        body,
        span,
    }
}

/// The keyword of a modifier for messages.
pub(crate) fn modifier_word(m: &Modifier<'_>) -> &'static str {
    match m {
        Modifier::Static(_) => "static",
        Modifier::Final(_) => "final",
        Modifier::Abstract(_) => "abstract",
        Modifier::Readonly(_) => "readonly",
        Modifier::Public(_) => "public",
        Modifier::PublicSet(_) => "public(set)",
        Modifier::Protected(_) => "protected",
        Modifier::ProtectedSet(_) => "protected(set)",
        Modifier::Private(_) => "private",
        Modifier::PrivateSet(_) => "private(set)",
    }
}
