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

    types::register_classes(r);
    class::register_classes(r);
    func::register_classes(r);
    prop::register_classes(r);
    attrs::register_classes(r);
    reference::register_classes(r);
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
