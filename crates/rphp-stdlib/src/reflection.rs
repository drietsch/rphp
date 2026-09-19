//! The Reflection extension (php-src `ext/reflection/php_reflection.c` plus
//! `Zend/zend_attributes.c` for `Attribute`).
//!
//! # The model
//!
//! Every reflector is an ordinary native class instance. What php shows in a
//! dump is a **real declared slot** — `ReflectionClass::$name`,
//! `ReflectionMethod::$name`/`$class`, `ReflectionProperty::$name`/`$class`,
//! `ReflectionClassConstant::$name`/`$class`, `ReflectionParameter::$name`,
//! `ReflectionAttribute::$name` — so `var_dump(new ReflectionClass(C::class))`
//! prints php's one-property object. Everything else the reflector needs is
//! hidden in the instance's native [`Payload`](rphp_value::Payload):
//!
//! | class | hidden state |
//! |---|---|
//! | `ReflectionClass` / `ReflectionObject` / `ReflectionEnum` | the class id, plus the instance for `ReflectionObject` |
//! | `ReflectionFunction` / `ReflectionMethod` | an `FnTarget`: a function id, a native id, a `(class, method)` pair or a closure |
//! | `ReflectionParameter` | its function plus the declaration position |
//! | `ReflectionProperty` / `ReflectionClassConstant` | the declaring class id and the member name |
//! | `ReflectionNamedType` / `ReflectionUnionType` / `ReflectionIntersectionType` | the decomposed type (no php-visible property at all, which is why php dumps them empty) |
//! | `ReflectionAttribute` | the compiled attribute |
//!
//! A reflector stores a *reference* rather than a resolved handle, so it
//! stays correct when the class table grows under it — and the state is
//! cloned out of the payload before anything re-enters the interpreter, so a
//! reflector method may call user code.
//!
//! # Two workarounds to know about
//!
//! `rphp-stdlib` depends on `rphp-runtime` and `rphp-value` only, so this
//! module cannot name `rphp_bytecode::TypeDecl`, `InitRef` or `AttrDef` even
//! though the values are reachable through `FuncRt::f`:
//!
//! * a **declared type** enters as `TypeDecl`'s `Display` output — php's own
//!   canonical spelling — and `reflection/types.rs` parses it back into a
//!   tree. The grammar is closed, so the round trip is exact;
//! * an **initializer** (`InitRef`) is read through its derived `Debug`
//!   (`Const(3)` / `Thunk(7)`) by `common::parse_init_ref`.
//!
//! Both are marked at their definition and should become a plain `match` as
//! soon as the crate may depend on `rphp-bytecode` (or the runtime re-exports
//! those three types).
//!
//! # Deliberately absent
//!
//! * the `__toString()` dumps (php's multi-line `Class [ <user> class Foo ]
//!   { … }` text) on every class but `ReflectionType`, whose `__toString` is
//!   the type spelling and is implemented;
//! * `ReflectionGenerator`, `ReflectionFiber`, `ReflectionReference`,
//!   `ReflectionExtension`, `ReflectionZendExtension`, the `Reflection`
//!   helper class, and the php 8.4 lazy-object API
//!   (`newLazyGhost`/`newLazyProxy`/`initializeLazyObject`/…).
//!
//! Per-class divergences are documented in each submodule's header.

mod attrs;
mod class;
mod common;
mod func;
mod prop;
mod reference;
mod types;

use rphp_runtime::{NativeFn, Registry};

/// Functions this module provides. The Reflection extension is classes only.
pub(crate) static FUNCTIONS: &[NativeFn] = &[];

/// Constants this module provides. Every Reflection constant is a class
/// constant (`ReflectionMethod::IS_STATIC`, `Attribute::TARGET_ALL`, …),
/// declared with its class, so there is nothing global to add.
pub(crate) fn register_constants(_r: &mut Registry) {}

/// Register the whole extension. Parents first: `Reflector` before every
/// class that implements it, `Exception` (already registered by
/// `zend_exceptions`) before `ReflectionException`, `ReflectionClass` before
/// `ReflectionObject`/`ReflectionEnum`, and so on.
pub(crate) fn register_classes(r: &mut Registry) {
    if r.interp().class_by_name(b"ReflectionClass").is_some() {
        return;
    }

    // `Reflector` is `interface Reflector extends Stringable`; the dump
    // format behind `__toString()` is out of scope, so the signature is
    // declared and no class implements it. `$r instanceof Reflector` — which
    // is what callers actually test — answers correctly either way.
    r.interface("Reflector")
        .implements(&["Stringable"])
        .abstract_method("__toString", common::sig(0, Some(0)))
        .finish();

    r.class("ReflectionException").extends("Exception").finish();

    // `Reflection` itself is just the modifier-name helper, and
    // `SensitiveParameter` is the attribute php marks arguments with so a
    // backtrace redacts them (the engine drops attributes, so the class only
    // has to exist and be final).
    r.class("Reflection")
        .method(
            "getModifierNames",
            common::snm(1, Some(1), get_modifier_names),
        )
        .finish();
    r.class("SensitiveParameter")
        .flags(rphp_runtime::ClassFlags::FINAL)
        .finish();
    // `ReturnTypeWillChange` is Zend's own attribute, the one that silences
    // the "return type should be compatible" deprecation on an internal
    // interface's method. It is declared here beside the other attribute
    // class for the same reason: the engine drops attributes, so it only
    // has to exist.
    r.class("ReturnTypeWillChange")
        .flags(rphp_runtime::ClassFlags::FINAL)
        .finish();

    types::register_classes(r);
    class::register_classes(r);
    func::register_classes(r);
    prop::register_classes(r);
    attrs::register_classes(r);
    reference::register_classes(r);
}

/// `Reflection::getModifierNames(int $modifiers): array` — php's spelling of
/// each bit, in php's order (abstract, final, then the visibility pair, then
/// static and readonly).
fn get_modifier_names(
    _: &mut rphp_runtime::Ctx,
    _: Option<&rphp_value::Object>,
    args: &mut [rphp_value::Value],
) -> rphp_runtime::NativeResult {
    let m = args[0].deref().to_int();
    let mut out = rphp_value::Array::new();
    for (bit, name) in [
        (common::IS_ABSTRACT, "abstract"),
        (common::IS_FINAL, "final"),
        (common::IS_PUBLIC, "public"),
        (common::IS_PROTECTED, "protected"),
        (common::IS_PRIVATE, "private"),
        (common::IS_PROTECTED_SET, "protected(set)"),
        (common::IS_PRIVATE_SET, "private(set)"),
        (common::IS_STATIC, "static"),
        (common::IS_READONLY, "readonly"),
    ] {
        if m & bit != 0 {
            out.push(rphp_value::Value::string(name.as_bytes()));
        }
    }
    Ok(rphp_value::Value::Array(out))
}

#[cfg(test)]
mod tests {
    use crate::tests::interp;

    #[test]
    fn the_class_tree_matches_php() {
        let it = interp();
        let id = |n: &[u8]| it.class_by_name(n).expect("registered");
        assert!(it.instanceof_class(id(b"ReflectionObject"), id(b"ReflectionClass")));
        assert!(it.instanceof_class(id(b"ReflectionEnum"), id(b"ReflectionClass")));
        assert!(it.instanceof_class(id(b"ReflectionMethod"), id(b"ReflectionFunctionAbstract")));
        assert!(it.instanceof_class(id(b"ReflectionFunction"), id(b"ReflectionFunctionAbstract")));
        assert!(it.instanceof_class(id(b"ReflectionNamedType"), id(b"ReflectionType")));
        assert!(it.instanceof_class(id(b"ReflectionUnionType"), id(b"ReflectionType")));
        assert!(it.instanceof_class(id(b"ReflectionIntersectionType"), id(b"ReflectionType")));
        assert!(it.instanceof_class(
            id(b"ReflectionEnumBackedCase"),
            id(b"ReflectionEnumUnitCase")
        ));
        assert!(it.instanceof_class(
            id(b"ReflectionEnumUnitCase"),
            id(b"ReflectionClassConstant")
        ));
        assert!(it.instanceof_class(id(b"ReflectionClass"), id(b"Reflector")));
        assert!(it.instanceof_class(id(b"ReflectionException"), id(b"Exception")));
        // php dumps a `ReflectionClass` with exactly one property.
        assert_eq!(it.class(id(b"ReflectionClass")).props.len(), 1);
        assert_eq!(it.class(id(b"ReflectionMethod")).props.len(), 2);
        assert_eq!(it.class(id(b"ReflectionNamedType")).props.len(), 0);
    }

    #[test]
    fn modifier_constants_match_php() {
        let mut it = interp();
        let cid = it.class_by_name(b"ReflectionProperty").unwrap();
        let get = |it: &mut rphp_runtime::Interp, n: &[u8]| {
            it.class_const(cid, n, None).unwrap().to_int()
        };
        assert_eq!(get(&mut it, b"IS_PUBLIC"), 1);
        assert_eq!(get(&mut it, b"IS_READONLY"), 128);
        assert_eq!(get(&mut it, b"IS_PRIVATE_SET"), 4096);
        let aid = it.class_by_name(b"Attribute").unwrap();
        assert_eq!(
            it.class_const(aid, b"TARGET_ALL", None).unwrap().to_int(),
            127
        );
        assert_eq!(
            it.class_const(aid, b"TARGET_CONSTANT", None)
                .unwrap()
                .to_int(),
            64
        );
        assert_eq!(
            it.class_const(aid, b"IS_REPEATABLE", None)
                .unwrap()
                .to_int(),
            128
        );
    }
}
