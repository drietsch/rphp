//! `ReflectionType` and its three concrete kinds (php-src
//! `ext/reflection/php_reflection.c`, `zend_type_to_string`).
//!
//! **How a type gets here.** The engine models a declaration as a
//! `rphp_bytecode::TypeDecl` whose `Display` already prints php's canonical
//! spelling — class names in declaration order, then the builtin keywords in
//! `zend_type_to_string`'s order, then `null` as a `?` prefix or a trailing
//! member. `rphp-stdlib` may not name that type (it does not depend on
//! `rphp-bytecode`), so a declaration enters this module as that canonical
//! string and [`parse`] takes it apart again. The grammar is closed — php
//! identifiers hold no `|`, `&`, `?` or parentheses — so the round trip is
//! exact, and the resulting tree is what every method here answers from.
//!
//! **What each object holds.** A reflector of any kind keeps a [`TypeState`]
//! in its payload; none of the three classes declares a php-visible property,
//! which is why php dumps them as `object(ReflectionNamedType)#7 (0) {}`.
//!
//! **`self` and `parent` are resolved on the way in**, as php does: a method
//! declared `function f(): self` in `SS` reports `getName() === "SS"` and
//! `isBuiltin() === false`. `static` is left as the keyword `static`, also
//! non-builtin.

use rphp_runtime::{nm, Ctx, NativeResult, Registry, Unwind};
use rphp_value::{Object, Value};

use super::common::{list, new_reflector, state, this};

/// Which of the three concrete classes a type is.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum TypeKind {
    /// `ReflectionNamedType`
    Named,
    /// `ReflectionUnionType`
    Union,
    /// `ReflectionIntersectionType`
    Intersection,
}

/// A decomposed declared type.
#[derive(Clone)]
pub(crate) struct TypeState {
    /// Which concrete class renders it.
    pub kind: TypeKind,
    /// The type name (`Named` only), with `self`/`parent` already resolved.
    pub name: Box<[u8]>,
    /// Whether the type admits `null`.
    pub nullable: bool,
    /// Whether `name` is a builtin keyword rather than a class name
    /// (`Named` only). `static`, `self` and `parent` are *not* builtin.
    pub builtin: bool,
    /// The members of a union or intersection, in php's order.
    pub members: Vec<TypeState>,
}

/// The keywords php reports as builtin. `static`, `self` and `parent` are
/// class-like and deliberately absent.
const BUILTINS: &[&str] = &[
    "int", "float", "string", "bool", "array", "object", "mixed", "void", "never", "null", "true",
    "false", "callable", "iterable",
];

/// Split `s` on `sep` at the top level, treating a parenthesized DNF group as
/// one atom.
fn split_top(s: &str, sep: char) -> Vec<&str> {
    let mut out = Vec::new();
    let mut depth = 0usize;
    let mut start = 0usize;
    for (i, c) in s.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            _ if c == sep && depth == 0 => {
                out.push(&s[start..i]);
                start = i + c.len_utf8();
            }
            _ => {}
        }
    }
    out.push(&s[start..]);
    out
}

/// A single name, with `self`/`parent` resolved against `scope`.
fn named(ctx: &Ctx, raw: &str, scope: Option<u32>, nullable: bool) -> TypeState {
    let lower = raw.to_ascii_lowercase();
    let resolved: Vec<u8> = match lower.as_str() {
        "self" => match scope {
            Some(c) => ctx.class(c).name.to_vec(),
            None => raw.as_bytes().to_vec(),
        },
        "parent" => match scope.and_then(|c| ctx.class(c).parent) {
            Some(p) => ctx.class(p).name.to_vec(),
            None => raw.as_bytes().to_vec(),
        },
        _ => raw.as_bytes().to_vec(),
    };
    let builtin = BUILTINS.contains(&lower.as_str());
    TypeState {
        kind: TypeKind::Named,
        name: resolved.into_boxed_slice(),
        // `mixed` and `null` admit null on their own.
        nullable: nullable || lower == "mixed" || lower == "null",
        builtin,
        members: Vec::new(),
    }
}

/// Parse php's canonical spelling of a type back into a tree.
pub(crate) fn parse(ctx: &Ctx, s: &str, scope: Option<u32>) -> TypeState {
    let s = s.trim();
    if let Some(rest) = s.strip_prefix('?') {
        // `?X` is only emitted for a single, non-parenthesized member.
        return named(ctx, rest, scope, true);
    }
    let parts = split_top(s, '|');
    if parts.len() > 1 {
        let members: Vec<TypeState> = parts
            .iter()
            .map(|p| {
                let p = p.trim();
                match p.strip_prefix('(').and_then(|q| q.strip_suffix(')')) {
                    Some(inner) => intersection(ctx, inner, scope),
                    None => named(ctx, p, scope, false),
                }
            })
            .collect();
        let nullable = members.iter().any(|m| m.nullable);
        return TypeState {
            kind: TypeKind::Union,
            name: Box::from(&b""[..]),
            nullable,
            builtin: false,
            members,
        };
    }
    let amp = split_top(s, '&');
    if amp.len() > 1 {
        return intersection(ctx, s, scope);
    }
    named(ctx, s, scope, false)
}

/// `A&B&…`.
fn intersection(ctx: &Ctx, s: &str, scope: Option<u32>) -> TypeState {
    let members: Vec<TypeState> = split_top(s, '&')
        .iter()
        .map(|p| named(ctx, p.trim(), scope, false))
        .collect();
    TypeState {
        kind: TypeKind::Intersection,
        name: Box::from(&b""[..]),
        nullable: false,
        builtin: false,
        members,
    }
}

/// php's spelling of a type tree — what `ReflectionType::__toString()` and a
/// `TypeError` print.
pub(crate) fn render(t: &TypeState) -> Vec<u8> {
    match t.kind {
        TypeKind::Named => {
            let name = String::from_utf8_lossy(&t.name).into_owned();
            // `mixed` and `null` already admit null; php never writes `?mixed`.
            if t.nullable && !t.builtin_null_like() {
                format!("?{name}").into_bytes()
            } else {
                name.into_bytes()
            }
        }
        TypeKind::Union => {
            let parts: Vec<String> = t
                .members
                .iter()
                .map(|m| {
                    let s = String::from_utf8_lossy(&render(m)).into_owned();
                    if m.kind == TypeKind::Intersection {
                        format!("({s})")
                    } else {
                        s
                    }
                })
                .collect();
            parts.join("|").into_bytes()
        }
        TypeKind::Intersection => {
            let parts: Vec<String> = t
                .members
                .iter()
                .map(|m| String::from_utf8_lossy(&render(m)).into_owned())
                .collect();
            parts.join("&").into_bytes()
        }
    }
}

impl TypeState {
    /// Whether the name is one that carries `null` by itself, so php prints
    /// it without a `?`.
    fn builtin_null_like(&self) -> bool {
        self.name.eq_ignore_ascii_case(b"mixed") || self.name.eq_ignore_ascii_case(b"null")
    }

    /// The class this type is rendered by.
    fn class_name(&self) -> &'static str {
        match self.kind {
            TypeKind::Named => "ReflectionNamedType",
            TypeKind::Union => "ReflectionUnionType",
            TypeKind::Intersection => "ReflectionIntersectionType",
        }
    }
}

/// Build the php object for a parsed type.
pub(crate) fn to_object(ctx: &mut Ctx, t: TypeState) -> Result<Value, Unwind> {
    let class = t.class_name();
    let o = new_reflector(ctx, class, t)?;
    Ok(Value::Object(o))
}

/// Build the php object for a declared type given its canonical spelling.
pub(crate) fn from_string(ctx: &mut Ctx, s: &str, scope: Option<u32>) -> Result<Value, Unwind> {
    let t = parse(ctx, s, scope);
    to_object(ctx, t)
}

// ---- methods ---------------------------------------------------------------

/// `ReflectionNamedType::getName(): string`
fn named_get_name(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let t: TypeState = state(this(o)?)?;
    Ok(Value::string(&t.name))
}

/// `ReflectionNamedType::isBuiltin(): bool`
fn named_is_builtin(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let t: TypeState = state(this(o)?)?;
    Ok(Value::Bool(t.builtin))
}

/// `ReflectionType::allowsNull(): bool`
fn type_allows_null(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let t: TypeState = state(this(o)?)?;
    Ok(Value::Bool(t.nullable))
}

/// `ReflectionType::__toString(): string`
fn type_to_string(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let t: TypeState = state(this(o)?)?;
    Ok(Value::string(&render(&t)))
}

/// `ReflectionUnionType::getTypes(): array` /
/// `ReflectionIntersectionType::getTypes(): array`
fn type_get_types(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let t: TypeState = state(this(o)?)?;
    let mut out = Vec::new();
    for m in t.members {
        out.push(to_object(ctx, m)?);
    }
    Ok(list(out))
}

/// Register `ReflectionType` and its three kinds.
pub(crate) fn register_classes(r: &mut Registry) {
    r.class("ReflectionType")
        .flags(rphp_runtime::ClassFlags::ABSTRACT)
        .implements(&["Stringable"])
        .method("allowsNull", nm!(0, Some(0), type_allows_null))
        .method("__toString", nm!(0, Some(0), type_to_string))
        .finish();

    r.class("ReflectionNamedType")
        .extends("ReflectionType")
        .method("getName", nm!(0, Some(0), named_get_name))
        .method("isBuiltin", nm!(0, Some(0), named_is_builtin))
        .finish();

    r.class("ReflectionUnionType")
        .extends("ReflectionType")
        .method("getTypes", nm!(0, Some(0), type_get_types))
        .finish();

    r.class("ReflectionIntersectionType")
        .extends("ReflectionType")
        .method("getTypes", nm!(0, Some(0), type_get_types))
        .finish();
}
