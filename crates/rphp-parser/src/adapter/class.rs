//! Class-likes (`class`, `interface`, `trait`, `enum`, anonymous classes),
//! their members, modifiers and trait adaptations.

use mago_span::HasSpan;
use mago_syntax::cst::{
    AnonymousClass, Class, ClassLikeConstant, ClassLikeMember, Enum, EnumCase as MagoEnumCase,
    EnumCaseItem, Extends, Identifier, Implements, Interface, Method, MethodBody, Modifier,
    Property, PropertyItem, Sequence, Trait, TraitUse as MagoTraitUse, TraitUseAdaptation,
    TraitUseMethodReference, TraitUseSpecification,
};
use rphp_ast::v2::{
    Adaptation, Builtin, ClassKind, ClassLike, ConstItem, ConstMember, EnumCase, Member,
    MethodDecl, Modifiers, Name, PropItem, PropMember, TraitUse, Visibility,
};
use rphp_span::Span;

use super::conformance::{check_const_expr, unexpected_token, ConstCtx};
use super::ctx::{ClassScope, Ctx, FnKind};
use super::func::{self, modifier_word, ParamOwner};
use super::types::{self, TypePos};
use super::{attrs, docs, names, stmt};

/// What a modifier sequence is attached to.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ModTarget {
    /// A class declaration.
    Class,
    /// An anonymous class.
    AnonClass,
    /// A method.
    Method,
    /// A property.
    Prop,
    /// A class constant.
    Const,
    /// A promoted parameter.
    Param,
}

/// Convert a modifier sequence, reporting duplicates and the modifiers PHP
/// forbids on this target.
pub(crate) fn modifiers(ctx: &mut Ctx<'_, '_>, seq: &Sequence<'_, Modifier<'_>>, target: ModTarget) -> Modifiers {
    let mut m = Modifiers::default();
    let mut span: Option<Span> = None;
    for md in seq.iter() {
        let s = ctx.sp(md.span());
        span = Some(span.map_or(s, |p| p.to(s)));
        let word = modifier_word(md);
        let forbidden = match (target, md) {
            (ModTarget::Class | ModTarget::AnonClass, Modifier::Static(_) | Modifier::Public(_) | Modifier::Protected(_) | Modifier::Private(_) | Modifier::PublicSet(_) | Modifier::ProtectedSet(_) | Modifier::PrivateSet(_)) => {
                ctx.reject(unexpected_token(word), s);
                true
            }
            (ModTarget::AnonClass, Modifier::Abstract(_) | Modifier::Final(_)) => {
                ctx.reject(format!("Cannot use the {word} modifier on an anonymous class"), s);
                true
            }
            (ModTarget::Method, Modifier::Readonly(_)) => {
                ctx.reject("Cannot use the readonly modifier on a method", s);
                true
            }
            (ModTarget::Method, Modifier::PublicSet(_) | Modifier::ProtectedSet(_) | Modifier::PrivateSet(_)) => {
                ctx.reject(format!("Cannot use the {word} modifier on a method"), s);
                true
            }
            (ModTarget::Const, Modifier::Static(_) | Modifier::Abstract(_) | Modifier::Readonly(_)) => {
                ctx.reject(format!("Cannot use the {word} modifier on a class constant"), s);
                true
            }
            (ModTarget::Const, Modifier::PublicSet(_) | Modifier::ProtectedSet(_) | Modifier::PrivateSet(_)) => {
                ctx.reject(format!("Cannot use the {word} modifier on a class constant"), s);
                true
            }
            _ => false,
        };
        if forbidden {
            continue;
        }
        match md {
            Modifier::Static(_) => {
                if m.static_ {
                    ctx.reject("Multiple static modifiers are not allowed", s);
                }
                m.static_ = true;
            }
            Modifier::Final(_) => {
                if m.final_ {
                    ctx.reject("Multiple final modifiers are not allowed", s);
                }
                m.final_ = true;
            }
            Modifier::Abstract(_) => {
                if m.abstract_ {
                    ctx.reject("Multiple abstract modifiers are not allowed", s);
                }
                m.abstract_ = true;
            }
            Modifier::Readonly(_) => {
                if m.readonly {
                    ctx.reject("Multiple readonly modifiers are not allowed", s);
                }
                m.readonly = true;
            }
            Modifier::Public(_) | Modifier::Protected(_) | Modifier::Private(_) => {
                if m.vis.is_some() {
                    ctx.reject("Multiple access type modifiers are not allowed", s);
                }
                m.vis = Some(match md {
                    Modifier::Public(_) => Visibility::Public,
                    Modifier::Protected(_) => Visibility::Protected,
                    _ => Visibility::Private,
                });
            }
            Modifier::PublicSet(_) | Modifier::ProtectedSet(_) | Modifier::PrivateSet(_) => {
                if m.set_vis.is_some() {
                    ctx.reject("Multiple access type modifiers are not allowed", s);
                }
                m.set_vis = Some(match md {
                    Modifier::PublicSet(_) => Visibility::Public,
                    Modifier::ProtectedSet(_) => Visibility::Protected,
                    _ => Visibility::Private,
                });
            }
        }
    }
    if m.abstract_ && m.final_ {
        let s = span.unwrap_or(Span::dummy());
        match target {
            ModTarget::Class => ctx.reject("Cannot use the final modifier on an abstract class", s),
            ModTarget::Method => ctx.reject("Cannot use the final modifier on an abstract method", s),
            _ => {}
        }
    }
    m.span = span.unwrap_or(Span::dummy());
    m
}

/// `true` inside a `readonly class` body (properties are implicitly readonly).
pub(crate) fn in_readonly_class(ctx: &Ctx<'_, '_>) -> bool {
    ctx.classes.last().is_some_and(|c| c.readonly)
}

/// Per-class facts the member conversions need.
struct Info {
    kind: ClassKind,
    name: String,
    is_abstract: bool,
    is_anon: bool,
}

fn extends(ctx: &mut Ctx<'_, '_>, e: Option<&Extends<'_>>) -> Vec<Name> {
    e.map(|e| e.types.iter().map(|t| names::name(ctx, t)).collect())
        .unwrap_or_default()
}

fn implements(ctx: &mut Ctx<'_, '_>, i: Option<&Implements<'_>>) -> Vec<Name> {
    i.map(|i| i.types.iter().map(|t| names::name(ctx, t)).collect())
        .unwrap_or_default()
}

/// A `class` declaration.
pub(crate) fn class(ctx: &mut Ctx<'_, '_>, c: &Class<'_>) -> ClassLike {
    let span = ctx.sp(c.span());
    let after_attrs = c
        .modifiers
        .first()
        .map_or(c.class.span.start.offset, |m| m.span().start.offset);
    let doc = docs::decl_doc(ctx, &c.attribute_lists, after_attrs);
    let attrs = attrs::groups(ctx, &c.attribute_lists);
    let modifiers = modifiers(ctx, &c.modifiers, ModTarget::Class);
    names::check_reserved(ctx, &c.name, "identifier");
    names::check_readonly_name(ctx, &c.name);
    let name = names::local(ctx, &c.name);
    let ext = extends(ctx, c.extends.as_ref());
    if ext.len() > 1 {
        ctx.reject(format!("{}, expecting \"{{\"", unexpected_token(",")), ext[1].span);
    }
    let imp = implements(ctx, c.implements.as_ref());
    let info = Info {
        kind: ClassKind::Class,
        name: String::from_utf8_lossy(c.name.value).into_owned(),
        is_abstract: modifiers.abstract_,
        is_anon: false,
    };
    let members = members(ctx, &c.members, &info, !ext.is_empty(), modifiers.readonly);
    ClassLike {
        kind: ClassKind::Class,
        attrs,
        modifiers,
        name: Some(name),
        extends: ext,
        implements: imp,
        backing: None,
        members,
        doc,
        span,
    }
}

/// An anonymous class (`new class(...) extends X { }`).
pub(crate) fn anonymous(ctx: &mut Ctx<'_, '_>, a: &AnonymousClass<'_>) -> ClassLike {
    let span = ctx.sp(a.span());
    let doc = docs::decl_doc(ctx, &a.attribute_lists, a.new.span.start.offset);
    let attrs = attrs::groups(ctx, &a.attribute_lists);
    let modifiers = modifiers(ctx, &a.modifiers, ModTarget::AnonClass);
    let ext = extends(ctx, a.extends.as_ref());
    let imp = implements(ctx, a.implements.as_ref());
    let info = Info {
        kind: ClassKind::Class,
        name: "class@anonymous".into(),
        is_abstract: false,
        is_anon: true,
    };
    let members = members(ctx, &a.members, &info, !ext.is_empty(), modifiers.readonly);
    ClassLike {
        kind: ClassKind::Class,
        attrs,
        modifiers,
        name: None,
        extends: ext,
        implements: imp,
        backing: None,
        members,
        doc,
        span,
    }
}

/// An `interface` declaration.
pub(crate) fn interface(ctx: &mut Ctx<'_, '_>, i: &Interface<'_>) -> ClassLike {
    let span = ctx.sp(i.span());
    let doc = docs::decl_doc(ctx, &i.attribute_lists, i.interface.span.start.offset);
    let attrs = attrs::groups(ctx, &i.attribute_lists);
    names::check_reserved(ctx, &i.name, "identifier");
    names::check_readonly_name(ctx, &i.name);
    let name = names::local(ctx, &i.name);
    let ext = extends(ctx, i.extends.as_ref());
    let info = Info {
        kind: ClassKind::Interface,
        name: String::from_utf8_lossy(i.name.value).into_owned(),
        is_abstract: true,
        is_anon: false,
    };
    let members = members(ctx, &i.members, &info, false, false);
    ClassLike {
        kind: ClassKind::Interface,
        attrs,
        modifiers: Modifiers::default(),
        name: Some(name),
        extends: ext,
        implements: Vec::new(),
        backing: None,
        members,
        doc,
        span,
    }
}

/// A `trait` declaration.
pub(crate) fn trait_(ctx: &mut Ctx<'_, '_>, t: &Trait<'_>) -> ClassLike {
    let span = ctx.sp(t.span());
    let doc = docs::decl_doc(ctx, &t.attribute_lists, t.r#trait.span.start.offset);
    let attrs = attrs::groups(ctx, &t.attribute_lists);
    names::check_reserved(ctx, &t.name, "identifier");
    names::check_readonly_name(ctx, &t.name);
    let name = names::local(ctx, &t.name);
    let info = Info {
        kind: ClassKind::Trait,
        name: String::from_utf8_lossy(t.name.value).into_owned(),
        is_abstract: true,
        is_anon: false,
    };
    let members = members(ctx, &t.members, &info, false, false);
    ClassLike {
        kind: ClassKind::Trait,
        attrs,
        modifiers: Modifiers::default(),
        name: Some(name),
        extends: Vec::new(),
        implements: Vec::new(),
        backing: None,
        members,
        doc,
        span,
    }
}

/// An `enum` declaration.
pub(crate) fn enum_(ctx: &mut Ctx<'_, '_>, e: &Enum<'_>) -> ClassLike {
    let span = ctx.sp(e.span());
    let doc = docs::decl_doc(ctx, &e.attribute_lists, e.r#enum.span.start.offset);
    let attrs = attrs::groups(ctx, &e.attribute_lists);
    names::check_reserved(ctx, &e.name, "identifier");
    names::check_readonly_name(ctx, &e.name);
    let name = names::local(ctx, &e.name);
    let backing = e.backing_type_hint.as_ref().map(|b| {
        let t = types::hint(ctx, &b.hint);
        if !(types::is_builtin(&t, Builtin::Int) || types::is_builtin(&t, Builtin::String)) {
            let shown = types::display(ctx, &t);
            ctx.reject(
                format!("Enum backing type must be int or string, {shown} given"),
                t.span,
            );
        }
        t
    });
    let imp = implements(ctx, e.implements.as_ref());
    let info = Info {
        kind: ClassKind::Enum,
        name: String::from_utf8_lossy(e.name.value).into_owned(),
        is_abstract: false,
        is_anon: false,
    };
    ctx.enum_backed = backing.is_some();
    let members = members(ctx, &e.members, &info, false, false);
    ClassLike {
        kind: ClassKind::Enum,
        attrs,
        modifiers: Modifiers::default(),
        name: Some(name),
        extends: Vec::new(),
        implements: imp,
        backing,
        members,
        doc,
        span,
    }
}

/// Convert a member list under a pushed class scope.
fn members(
    ctx: &mut Ctx<'_, '_>,
    seq: &Sequence<'_, ClassLikeMember<'_>>,
    info: &Info,
    has_parent: bool,
    readonly: bool,
) -> Vec<Member> {
    ctx.classes.push(ClassScope {
        kind: info.kind,
        has_parent,
        name: if info.is_anon { None } else { Some(info.name.clone()) },
        is_abstract: info.is_abstract,
        readonly,
    });
    let mut seen_methods: Vec<String> = Vec::new();
    let mut seen_props: Vec<Vec<u8>> = Vec::new();
    let mut seen_consts: Vec<Vec<u8>> = Vec::new();
    let mut abstract_hooks: Vec<String> = Vec::new();
    let mut out = Vec::with_capacity(seq.len());
    for m in seq.iter() {
        let member = match m {
            ClassLikeMember::TraitUse(t) => Member::TraitUse(trait_use(ctx, t, info)),
            ClassLikeMember::Constant(c) => Member::Const(constant(ctx, c, info, &mut seen_consts)),
            ClassLikeMember::Property(p) => {
                let prop = property(ctx, p, info, &mut seen_props);
                if prop.modifiers.abstract_ && info.kind == ClassKind::Class && !info.is_abstract {
                    for h in &prop.hooks {
                        if matches!(h.body, rphp_ast::v2::HookBody::Abstract) {
                            let pname = prop
                                .items
                                .first()
                                .map(|i| ctx.interner.resolve_lossy(i.name).into_owned())
                                .unwrap_or_default();
                            abstract_hooks.push(format!("{}::${pname}::{}", info.name, h.kind.as_str()));
                        }
                    }
                }
                Member::Prop(prop)
            }
            ClassLikeMember::EnumCase(c) => Member::EnumCase(enum_case(ctx, c, info, &mut seen_consts)),
            ClassLikeMember::Method(m) => Member::Method(method(ctx, m, info, &mut seen_methods)),
        };
        out.push(member);
    }
    if !abstract_hooks.is_empty() {
        let n = abstract_hooks.len();
        let plural = if n == 1 { "" } else { "s" };
        let span = out.last().map_or(Span::dummy(), |m| m.span());
        ctx.reject(
            format!(
                "Class {} contains {n} abstract method{plural} and must therefore be declared abstract or implement the remaining method{plural} ({})",
                info.name,
                abstract_hooks.join(", ")
            ),
            span,
        );
    }
    ctx.classes.pop();
    out
}

fn trait_use(ctx: &mut Ctx<'_, '_>, t: &MagoTraitUse<'_>, info: &Info) -> TraitUse {
    let span = ctx.sp(t.span());
    let traits: Vec<Name> = t.trait_names.iter().map(|n| names::name(ctx, n)).collect();
    if info.kind == ClassKind::Interface {
        let first = t
            .trait_names
            .first()
            .map(|n| String::from_utf8_lossy(ident_text(n)).into_owned())
            .unwrap_or_default();
        ctx.reject(
            format!("Cannot use traits inside of interfaces. {first} is used in {}", info.name),
            span,
        );
    }
    let mut adaptations = Vec::new();
    if let TraitUseSpecification::Concrete(spec) = &t.specification {
        for a in spec.adaptations.iter() {
            let a_span = ctx.sp(a.span());
            match a {
                TraitUseAdaptation::Precedence(p) => {
                    let trait_ = names::name(ctx, &p.method_reference.trait_name);
                    let method = names::local_recorded(ctx, &p.method_reference.method_name);
                    let insteadof = p.trait_names.iter().map(|n| names::name(ctx, n)).collect();
                    adaptations.push(Adaptation::Precedence {
                        trait_,
                        method,
                        insteadof,
                        span: a_span,
                    });
                }
                TraitUseAdaptation::Alias(al) => {
                    let (trait_, method) = match &al.method_reference {
                        TraitUseMethodReference::Identifier(id) => (None, names::local_recorded(ctx, id)),
                        TraitUseMethodReference::Absolute(abs) => (
                            Some(names::name(ctx, &abs.trait_name)),
                            names::local_recorded(ctx, &abs.method_name),
                        ),
                    };
                    let vis = match &al.modifier {
                        None => None,
                        Some(Modifier::Public(_)) => Some(Visibility::Public),
                        Some(Modifier::Protected(_)) => Some(Visibility::Protected),
                        Some(Modifier::Private(_)) => Some(Visibility::Private),
                        // `T::m as final;` is legal; AST v2 has no slot for it
                        // (the alias keeps its visibility unchanged).
                        Some(Modifier::Final(_)) => None,
                        Some(Modifier::Readonly(k)) => {
                            let s = ctx.sp(k.span);
                            ctx.reject("Cannot use the readonly modifier on a method", s);
                            None
                        }
                        Some(other) => {
                            let s = ctx.sp(other.span());
                            ctx.reject(
                                format!(
                                    "Cannot use \"{}\" as method modifier in trait alias",
                                    modifier_word(other)
                                ),
                                s,
                            );
                            None
                        }
                    };
                    let alias = al.alias.as_ref().map(|a| names::local_recorded(ctx, a));
                    adaptations.push(Adaptation::Alias {
                        trait_,
                        method,
                        alias,
                        vis,
                        span: a_span,
                    });
                }
            }
        }
    }
    TraitUse {
        traits,
        adaptations,
        span,
    }
}

/// Zend's magic-method signature rules (`zend_check_magic_method_implementation`).
fn magic_method(
    ctx: &mut Ctx<'_, '_>,
    lower: &str,
    display: &str,
    modifiers: &Modifiers,
    params: &mago_syntax::cst::FunctionLikeParameterList<'_>,
    ret: Option<&rphp_ast::v2::Type>,
) {
    if !lower.starts_with("__") {
        return;
    }
    let nparams = params.parameters.len();
    let variadic = params.parameters.iter().any(|p| p.ellipsis.is_some());
    let by_ref = params.parameters.iter().any(|p| p.ampersand.is_some());
    let pspan = ctx.sp(params.span());
    let arity = |ctx: &mut Ctx<'_, '_>, n: usize| {
        if nparams != n || variadic {
            let s = if n == 1 { "" } else { "s" };
            ctx.reject(format!("Method {display}() must take exactly {n} argument{s}"), pspan);
        }
    };
    let no_args = |ctx: &mut Ctx<'_, '_>| {
        if nparams != 0 {
            ctx.reject(format!("Method {display}() cannot take arguments"), pspan);
        }
    };
    let non_static = |ctx: &mut Ctx<'_, '_>| {
        if modifiers.static_ {
            ctx.reject(format!("Method {display}() cannot be static"), modifiers.span);
        }
    };
    let must_static = |ctx: &mut Ctx<'_, '_>| {
        if !modifiers.static_ {
            ctx.reject(format!("Method {display}() must be static"), modifiers.span);
        }
    };
    let ret_must = |ctx: &mut Ctx<'_, '_>, want: &str, ok: &dyn Fn(&rphp_ast::v2::Type) -> bool| {
        if let Some(r) = ret {
            if !ok(r) && !types::is_builtin(r, Builtin::Never) {
                ctx.reject(format!("{display}(): Return type must be {want} when declared"), r.span);
            }
        }
    };
    let is = |b: Builtin| move |t: &rphp_ast::v2::Type| types::is_builtin(t, b);
    match lower {
        "__construct" | "__destruct" => {
            if let Some(r) = ret {
                ctx.reject(format!("Method {display}() cannot declare a return type"), r.span);
            }
            non_static(ctx);
            if lower == "__destruct" {
                no_args(ctx);
            }
        }
        "__clone" => {
            non_static(ctx);
            no_args(ctx);
            ret_must(ctx, "void", &is(Builtin::Void));
        }
        "__tostring" => {
            non_static(ctx);
            no_args(ctx);
            ret_must(ctx, "string", &is(Builtin::String));
        }
        "__get" | "__isset" | "__unset" => {
            non_static(ctx);
            arity(ctx, 1);
            match lower {
                "__isset" => ret_must(ctx, "bool", &is(Builtin::Bool)),
                "__unset" => ret_must(ctx, "void", &is(Builtin::Void)),
                _ => {}
            }
        }
        "__set" | "__call" => {
            non_static(ctx);
            arity(ctx, 2);
            if lower == "__set" {
                ret_must(ctx, "void", &is(Builtin::Void));
            }
        }
        "__callstatic" => {
            must_static(ctx);
            arity(ctx, 2);
        }
        "__invoke" => non_static(ctx),
        "__debuginfo" => {
            non_static(ctx);
            no_args(ctx);
            // `array`, `?array`, `array|null` / `null|array`.
            ret_must(ctx, "?array", &|t| match &t.kind {
                rphp_ast::v2::TypeKind::Nullable(i) => types::is_builtin(i, Builtin::Array),
                rphp_ast::v2::TypeKind::Union(ms) => ms.iter().all(|m| {
                    types::is_builtin(m, Builtin::Array) || types::is_builtin(m, Builtin::Null)
                }),
                _ => types::is_builtin(t, Builtin::Array),
            });
        }
        "__serialize" | "__sleep" => {
            non_static(ctx);
            no_args(ctx);
            ret_must(ctx, "array", &is(Builtin::Array));
        }
        "__unserialize" => {
            non_static(ctx);
            arity(ctx, 1);
            ret_must(ctx, "void", &is(Builtin::Void));
        }
        "__wakeup" => {
            non_static(ctx);
            no_args(ctx);
            ret_must(ctx, "void", &is(Builtin::Void));
        }
        "__set_state" => {
            must_static(ctx);
            arity(ctx, 1);
            // Any object-compatible type: `object`, a class name, `self`/`static`/`parent`.
            fn objectish(t: &rphp_ast::v2::Type) -> bool {
                match &t.kind {
                    rphp_ast::v2::TypeKind::Named(_) => true,
                    rphp_ast::v2::TypeKind::Builtin(
                        Builtin::Object | Builtin::SelfTy | Builtin::StaticTy | Builtin::ParentTy,
                    ) => true,
                    rphp_ast::v2::TypeKind::Union(ms) | rphp_ast::v2::TypeKind::Intersection(ms) => {
                        ms.iter().all(objectish)
                    }
                    _ => false,
                }
            }
            ret_must(ctx, "object", &objectish);
        }
        _ => return,
    }
    if by_ref && matches!(lower, "__get" | "__set" | "__isset" | "__unset" | "__call" | "__callstatic" | "__unserialize" | "__set_state") {
        ctx.reject(format!("Method {display}() cannot take arguments by reference"), pspan);
    }
}

fn ident_text<'a>(id: &Identifier<'a>) -> &'a [u8] {
    match id {
        Identifier::Local(l) => l.value,
        Identifier::Qualified(q) => q.value,
        Identifier::FullyQualified(f) => f.value,
    }
}

fn constant(
    ctx: &mut Ctx<'_, '_>,
    c: &ClassLikeConstant<'_>,
    info: &Info,
    seen: &mut Vec<Vec<u8>>,
) -> ConstMember {
    let span = ctx.sp(c.span());
    let after_attrs = c
        .modifiers
        .first()
        .map_or(c.r#const.span.start.offset, |m| m.span().start.offset);
    let doc = docs::decl_doc(ctx, &c.attribute_lists, after_attrs);
    let attrs = attrs::groups(ctx, &c.attribute_lists);
    let modifiers = modifiers(ctx, &c.modifiers, ModTarget::Const);
    let ty = c.hint.as_ref().map(|h| types::hint(ctx, h));
    let mut items = Vec::with_capacity(c.items.len());
    for it in c.items.iter() {
        let owner = format!("{}::{}", info.name, String::from_utf8_lossy(it.name.value));
        if let Some(t) = &ty {
            types::check(ctx, t, TypePos::Const, &owner);
        }
        if info.kind == ClassKind::Interface
            && matches!(modifiers.vis, Some(Visibility::Private | Visibility::Protected))
        {
            ctx.reject(
                format!("Access type for interface constant {owner} must be public"),
                modifiers.span,
            );
        }
        if super::ctx::eq_ci(it.name.value, "class") {
            ctx.reject(
                "A class constant must not be called 'class'; it is reserved for class name fetching",
                ctx.sp(it.name.span),
            );
        }
        if seen.contains(&it.name.value.to_vec()) {
            ctx.reject(format!("Cannot redefine class constant {owner}"), ctx.sp(it.name.span));
        }
        seen.push(it.name.value.to_vec());
        let name = names::local_recorded(ctx, &it.name);
        let value = super::expr::const_expr(ctx, it.value);
        check_const_expr(ctx, &value, ConstCtx::ClassConst);
        items.push(ConstItem {
            name,
            value,
            span: ctx.sp(it.span()),
        });
    }
    ConstMember {
        attrs,
        modifiers,
        ty,
        items,
        doc,
        span,
    }
}

/// The parts of a property declaration shared by the plain and hooked forms.
type PropParts<'x, 'a> = (
    &'x Sequence<'a, AttributeList<'a>>,
    &'x Sequence<'a, Modifier<'a>>,
    Option<u32>,
    Option<&'x mago_syntax::cst::Hint<'a>>,
    Vec<&'x PropertyItem<'a>>,
    Option<&'x mago_syntax::cst::PropertyHookList<'a>>,
);

fn property(ctx: &mut Ctx<'_, '_>, p: &Property<'_>, info: &Info, seen: &mut Vec<Vec<u8>>) -> PropMember {
    let span = ctx.sp(p.span());
    let (attr_lists, mod_seq, var, hint, items, hook_list): PropParts<'_, '_> = match p {
        Property::Plain(pl) => (
            &pl.attribute_lists,
            &pl.modifiers,
            pl.var.as_ref().map(|k| k.span.start.offset),
            pl.hint.as_ref(),
            pl.items.iter().collect(),
            None,
        ),
        Property::Hooked(h) => (
            &h.attribute_lists,
            &h.modifiers,
            h.var.as_ref().map(|k| k.span.start.offset),
            h.hint.as_ref(),
            vec![&h.item],
            Some(&h.hook_list),
        ),
    };
    let after_attrs = mod_seq
        .first()
        .map(|m| m.span().start.offset)
        .or(var)
        .or_else(|| hint.map(|h| h.span().start.offset))
        .or_else(|| items.first().map(|i| i.span().start.offset))
        .unwrap_or(span.lo);
    let doc = docs::decl_doc(ctx, attr_lists, after_attrs);
    let attrs = attrs::groups(ctx, attr_lists);
    let modifiers = modifiers(ctx, mod_seq, ModTarget::Prop);
    if var.is_some() && !mod_seq.is_empty() {
        ctx.reject(format!("{}, expecting variable", unexpected_token("var")), span);
    }
    let ty = hint.map(|h| types::hint(ctx, h));
    let readonly = modifiers.readonly || in_readonly_class(ctx);
    match info.kind {
        ClassKind::Interface if hook_list.is_none() => {
            ctx.reject("Interfaces may only include hooked properties", span);
        }
        ClassKind::Interface => {
            if matches!(modifiers.vis, Some(Visibility::Private | Visibility::Protected)) {
                ctx.reject("Property in interface cannot be protected or private", modifiers.span);
            }
        }
        ClassKind::Enum => ctx.reject(format!("Enum {} cannot include properties", info.name), span),
        _ => {}
    }
    if modifiers.abstract_ && hook_list.is_none() {
        ctx.reject("Only hooked properties may be declared abstract", modifiers.span);
    }
    if modifiers.final_ && modifiers.vis == Some(Visibility::Private) {
        ctx.reject("Property cannot be both final and private", modifiers.span);
    }
    let mut out_items = Vec::with_capacity(items.len());
    for it in items {
        let (var_tok, default) = match it {
            PropertyItem::Abstract(a) => (&a.variable, None),
            PropertyItem::Concrete(c) => (&c.variable, Some(c.value)),
        };
        let owner = format!("{}::{}", info.name, String::from_utf8_lossy(var_tok.name));
        if let Some(t) = &ty {
            types::check(ctx, t, TypePos::Prop, &owner);
        }
        if readonly {
            if ty.is_none() {
                ctx.reject(format!("Readonly property {owner} must have type"), span);
            }
            if default.is_some() {
                ctx.reject(format!("Readonly property {owner} cannot have default value"), span);
            }
            if modifiers.static_ {
                ctx.reject(format!("Static property {owner} cannot be readonly"), span);
            }
        }
        if modifiers.set_vis.is_some() && ty.is_none() {
            ctx.reject(
                format!("Property with asymmetric visibility {owner} must have type"),
                span,
            );
        }
        if hook_list.is_some() && modifiers.static_ {
            ctx.reject("Cannot declare hooks for static property", span);
        }
        if hook_list.is_some() && readonly {
            ctx.reject("Hooked properties cannot be readonly", span);
        }
        let raw = var_tok.name.strip_prefix(b"$").unwrap_or(var_tok.name).to_vec();
        if seen.contains(&raw) {
            ctx.reject(format!("Cannot redeclare {owner}"), ctx.sp(var_tok.span));
        }
        seen.push(raw);
        let name = ctx.intern_var(var_tok.name);
        let default = default.map(|d| {
            let e = super::expr::const_expr(ctx, d);
            check_const_expr(ctx, &e, ConstCtx::PropDefault);
            e
        });
        out_items.push(PropItem {
            name,
            default,
            span: ctx.sp(it.span()),
        });
    }
    let hooks = match hook_list {
        Some(list) => {
            let owner = out_items
                .first()
                .map(|i| format!("{}::${}", info.name, ctx.interner.resolve_lossy(i.name)))
                .unwrap_or_default();
            let abstract_ok = modifiers.abstract_ || info.kind == ClassKind::Interface;
            let private = modifiers.vis == Some(Visibility::Private);
            func::hooks(ctx, list, &owner, abstract_ok, info.kind == ClassKind::Interface, private)
        }
        None => Vec::new(),
    };
    PropMember {
        attrs,
        modifiers,
        ty,
        items: out_items,
        hooks,
        doc,
        span,
    }
}

use mago_syntax::cst::AttributeList;

fn enum_case(ctx: &mut Ctx<'_, '_>, c: &MagoEnumCase<'_>, info: &Info, seen: &mut Vec<Vec<u8>>) -> EnumCase {
    let span = ctx.sp(c.span());
    let doc = docs::decl_doc(ctx, &c.attribute_lists, c.case.span.start.offset);
    let attrs = attrs::groups(ctx, &c.attribute_lists);
    if info.kind != ClassKind::Enum {
        ctx.reject("Case can only be used in enums", span);
    }
    let (name_tok, value) = match &c.item {
        EnumCaseItem::Unit(u) => (&u.name, None),
        EnumCaseItem::Backed(b) => (&b.name, Some(b.value)),
    };
    let shown = String::from_utf8_lossy(name_tok.value).into_owned();
    if super::ctx::eq_ci(name_tok.value, "class") {
        ctx.reject(
            "A class constant must not be called 'class'; it is reserved for class name fetching",
            ctx.sp(name_tok.span),
        );
    }
    if info.kind == ClassKind::Enum {
        match (ctx.enum_backed, value.is_some()) {
            (true, false) => ctx.reject(
                format!("Case {shown} of backed enum {} must have a value", info.name),
                span,
            ),
            (false, true) => ctx.reject(
                format!("Case {shown} of non-backed enum {} must not have a value", info.name),
                span,
            ),
            _ => {}
        }
    }
    if seen.contains(&name_tok.value.to_vec()) {
        ctx.reject(
            format!("Cannot redefine class constant {}::{shown}", info.name),
            ctx.sp(name_tok.span),
        );
    }
    seen.push(name_tok.value.to_vec());
    let name = names::local_recorded(ctx, name_tok);
    let value = value.map(|v| {
        let e = super::expr::const_expr(ctx, v);
        check_const_expr(ctx, &e, ConstCtx::EnumCase);
        e
    });
    EnumCase {
        attrs,
        name,
        value,
        doc,
        span,
    }
}

fn method(ctx: &mut Ctx<'_, '_>, m: &Method<'_>, info: &Info, seen: &mut Vec<String>) -> MethodDecl {
    let span = ctx.sp(m.span());
    let after_attrs = m
        .modifiers
        .first()
        .map_or(m.function.span.start.offset, |md| md.span().start.offset);
    let doc = docs::decl_doc(ctx, &m.attribute_lists, after_attrs);
    let attrs = attrs::groups(ctx, &m.attribute_lists);
    let mut modifiers = modifiers(ctx, &m.modifiers, ModTarget::Method);
    if m.modifiers.is_empty() {
        // Messages about missing modifiers point at the `function` keyword.
        modifiers.span = ctx.sp(m.function.span);
    }
    let name = names::local_recorded(ctx, &m.name);
    let shown = String::from_utf8_lossy(m.name.value).into_owned();
    let display = format!("{}::{shown}", info.name);
    let lower = shown.to_ascii_lowercase();
    if seen.contains(&lower) {
        ctx.reject(format!("Cannot redeclare {display}()"), ctx.sp(m.name.span));
    }
    seen.push(lower.clone());
    let has_body = matches!(m.body, MethodBody::Concrete(_));
    let is_ctor = lower == "__construct";
    match info.kind {
        ClassKind::Interface => {
            if has_body {
                ctx.reject(format!("Interface function {display}() cannot contain body"), span);
            }
            if matches!(modifiers.vis, Some(Visibility::Private | Visibility::Protected)) {
                ctx.reject(
                    format!("Access type for interface method {display}() must be public"),
                    modifiers.span,
                );
            }
            if modifiers.abstract_ {
                ctx.reject(format!("Interface method {display}() must not be abstract"), modifiers.span);
            }
            if modifiers.final_ {
                ctx.reject(format!("Interface method {display}() must not be final"), modifiers.span);
            }
        }
        ClassKind::Enum if modifiers.abstract_ => {
            ctx.reject(format!("Enum method {display}() must not be abstract"), modifiers.span);
        }
        _ => {
            if modifiers.abstract_ && has_body {
                ctx.reject(format!("Abstract function {display}() cannot contain body"), span);
            }
            if !modifiers.abstract_ && !has_body {
                ctx.reject(format!("Non-abstract method {display}() must contain body"), span);
            }
            if modifiers.abstract_
                && modifiers.vis == Some(Visibility::Private)
                && info.kind != ClassKind::Trait
            {
                ctx.reject(
                    format!("Abstract function {display}() cannot be declared private"),
                    modifiers.span,
                );
            }
            if modifiers.abstract_ && info.is_anon {
                ctx.reject(
                    format!("Anonymous class method {shown}() must not be abstract"),
                    modifiers.span,
                );
            } else if modifiers.abstract_ && info.kind == ClassKind::Class && !info.is_abstract {
                ctx.reject(
                    format!(
                        "Class {} declares abstract method {shown}() and must therefore be declared abstract",
                        info.name
                    ),
                    span,
                );
            }
        }
    }
    let ret = m.return_type_hint.as_ref().map(|r| types::hint(ctx, &r.hint));
    magic_method(ctx, &lower, &display, &modifiers, &m.parameter_list, ret.as_ref());
    ctx.push_fn(FnKind::Method, func::ret_kind(ret.as_ref()), Some(display.clone()));
    ctx.fn_scope().by_ref = m.ampersand.is_some();
    if let Some(t) = &ret {
        types::check(ctx, t, TypePos::Return, &display);
    }
    let params = func::params(
        ctx,
        &m.parameter_list,
        ParamOwner::Method {
            is_ctor,
            is_abstract: !has_body,
            class: &info.name,
        },
    );
    let body = match &m.body {
        MethodBody::Concrete(b) => Some(stmt::block_stmts(ctx, b)),
        MethodBody::Abstract(_) => None,
    };
    let scope = ctx.pop_fn();
    func::finish_fn(ctx, scope);
    MethodDecl {
        attrs,
        modifiers,
        by_ref: m.ampersand.is_some(),
        name,
        params,
        ret,
        body,
        doc,
        span,
    }
}
