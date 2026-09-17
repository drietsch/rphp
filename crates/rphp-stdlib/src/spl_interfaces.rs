//! The interfaces php's object model assumes exist (php-src
//! `Zend/zend_interfaces.c` + `ext/spl/spl_iterators.c`): `Traversable` and
//! its two children `Iterator` / `IteratorAggregate`, `ArrayAccess`,
//! `Countable`, `JsonSerializable`, `Serializable`, `SeekableIterator`, and
//! the enum interfaces `UnitEnum` / `BackedEnum`.
//!
//! These carry **signatures only**: declaring them is what makes
//! `class C implements Iterator` link, `instanceof Countable` answer, and
//! `catch`/type checks against an interface work. The bodies here are never
//! reached — an abstract method is refused by dispatch — so they all share
//! one handler that says so.
//!
//! `Stringable` is registered by `zend_exceptions` (it is `Throwable`'s
//! parent interface), so it is only declared here if that has not run.

use rphp_runtime::{Ctx, NativeMethod, NativeResult, Registry, Unwind};
use rphp_value::{Object, Value};

/// The body every abstract signature carries. An interface method has no
/// implementation; php refuses the call (`Cannot call abstract method
/// C::m()`) before a body could run, so reaching this is an engine bug
/// rather than a program error.
fn abstract_body(_: &mut Ctx, _: Option<&Object>, _: &mut [Value]) -> NativeResult {
    Err(Unwind::error("Cannot call abstract method"))
}

/// An instance-method signature: `sig(0, Some(0), &[])` is `m(): mixed`.
fn sig(min: u8, max: Option<u8>, params: &'static [&'static str]) -> NativeMethod {
    NativeMethod {
        handler: abstract_body,
        min_args: min,
        max_args: max,
        params,
        by_ref: 0,
        is_static: false,
        is_final: false,
    }
}

/// A `static` method signature (`UnitEnum::cases`, `BackedEnum::from`).
fn static_sig(min: u8, max: Option<u8>, params: &'static [&'static str]) -> NativeMethod {
    NativeMethod {
        is_static: true,
        ..sig(min, max, params)
    }
}

/// Whether `name` still needs declaring (nobody has registered it yet).
fn needs_declaring(r: &mut Registry, name: &[u8]) -> bool {
    r.interp().class_by_name(name).is_none()
}

/// Register the core interfaces. Parents first: `Traversable` before
/// `Iterator`, `Iterator` before `SeekableIterator`, `UnitEnum` before
/// `BackedEnum`.
pub(crate) fn register_classes(r: &mut Registry) {
    // Every declaration is guarded: another extension (or the engine
    // bootstrap) may already own one of these interfaces — `Stringable`
    // comes with the `Throwable` tree, `JsonSerializable` with `json` — and
    // whoever registers first keeps it. That makes this module purely
    // additive whatever the registration order turns out to be.
    if needs_declaring(r, b"Stringable") {
        r.interface("Stringable")
            .abstract_method("__toString", sig(0, Some(0), &[]))
            .finish();
    }

    // `Traversable` has no methods: it is the marker `foreach` tests, and php
    // forbids a user class implementing it directly (it must go through
    // `Iterator` or `IteratorAggregate`).
    if needs_declaring(r, b"Traversable") {
        r.interface("Traversable").finish();
    }

    if needs_declaring(r, b"Iterator") {
        r.interface("Iterator")
            .implements(&["Traversable"])
            .abstract_method("current", sig(0, Some(0), &[]))
            .abstract_method("next", sig(0, Some(0), &[]))
            .abstract_method("key", sig(0, Some(0), &[]))
            .abstract_method("valid", sig(0, Some(0), &[]))
            .abstract_method("rewind", sig(0, Some(0), &[]))
            .finish();
    }

    if needs_declaring(r, b"IteratorAggregate") {
        r.interface("IteratorAggregate")
            .implements(&["Traversable"])
            .abstract_method("getIterator", sig(0, Some(0), &[]))
            .finish();
    }

    if needs_declaring(r, b"ArrayAccess") {
        r.interface("ArrayAccess")
            .abstract_method("offsetExists", sig(1, Some(1), &["offset"]))
            .abstract_method("offsetGet", sig(1, Some(1), &["offset"]))
            .abstract_method("offsetSet", sig(2, Some(2), &["offset", "value"]))
            .abstract_method("offsetUnset", sig(1, Some(1), &["offset"]))
            .finish();
    }

    if needs_declaring(r, b"Countable") {
        r.interface("Countable")
            .abstract_method("count", sig(0, Some(0), &[]))
            .finish();
    }

    if needs_declaring(r, b"JsonSerializable") {
        r.interface("JsonSerializable")
            .abstract_method("jsonSerialize", sig(0, Some(0), &[]))
            .finish();
    }

    if needs_declaring(r, b"Serializable") {
        r.interface("Serializable")
            .abstract_method("serialize", sig(0, Some(0), &[]))
            .abstract_method("unserialize", sig(1, Some(1), &["data"]))
            .finish();
    }

    // `SeekableIterator` is the SPL extension of `Iterator` that
    // `ArrayIterator` and `SplObjectStorage` implement.
    if needs_declaring(r, b"SeekableIterator") {
        r.interface("SeekableIterator")
            .implements(&["Iterator"])
            .abstract_method("seek", sig(1, Some(1), &["offset"]))
            .finish();
    }

    // The enum interfaces. `cases`/`from`/`tryFrom` are static; the engine
    // synthesizes the bodies on every `enum` declaration (`enums.rs`), so if
    // it has already declared these, its versions stand.
    if needs_declaring(r, b"UnitEnum") {
        r.interface("UnitEnum")
            .abstract_method("cases", static_sig(0, Some(0), &[]))
            .finish();
    }
    if needs_declaring(r, b"BackedEnum") {
        r.interface("BackedEnum")
            .implements(&["UnitEnum"])
            .abstract_method("from", static_sig(1, Some(1), &["value"]))
            .abstract_method("tryFrom", static_sig(1, Some(1), &["value"]))
            .finish();
    }
}


#[cfg(test)]
mod tests {
    use crate::tests::interp;

    #[test]
    fn interfaces_are_declared_with_phps_parents() {
        let it = interp();
        let traversable = it.class_by_name(b"Traversable").unwrap();
        let iterator = it.class_by_name(b"Iterator").unwrap();
        let aggregate = it.class_by_name(b"IteratorAggregate").unwrap();
        let seekable = it.class_by_name(b"SeekableIterator").unwrap();
        assert!(it.instanceof_class(iterator, traversable));
        assert!(it.instanceof_class(aggregate, traversable));
        assert!(it.instanceof_class(seekable, iterator));
        assert!(it.instanceof_class(seekable, traversable));
        let backed = it.class_by_name(b"BackedEnum").unwrap();
        assert!(it.instanceof_class(backed, it.class_by_name(b"UnitEnum").unwrap()));
        assert_eq!(it.class(iterator).kind, rphp_runtime::ClassKind::Interface);
    }

    #[test]
    fn signatures_match_phps_arity_and_staticness() {
        let it = interp();
        let aa = it.class(it.class_by_name(b"ArrayAccess").unwrap());
        let set = aa.method(b"offsetset").unwrap();
        assert!(set.is_abstract);
        assert!(!set.is_static);
        let unit = it.class(it.class_by_name(b"UnitEnum").unwrap());
        assert!(unit.method(b"cases").unwrap().is_static);
        let backed = it.class(it.class_by_name(b"BackedEnum").unwrap());
        assert!(backed.method(b"from").unwrap().is_static);
        // `BackedEnum` inherits `cases` from `UnitEnum`.
        assert!(backed.method(b"cases").is_some());
        let count = it.class(it.class_by_name(b"Countable").unwrap());
        assert!(count.method(b"count").is_some());
    }
}
