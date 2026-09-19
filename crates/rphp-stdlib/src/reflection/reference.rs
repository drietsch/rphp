//! `ReflectionReference` (php-src `ext/reflection/php_reflection.c`): the one
//! reflector that is not about a declaration but about *storage* — whether an
//! array element is a php reference, and whether two elements are the same
//! one.
//!
//! php answers `getId()` with 20 opaque bytes rather than the address, so a
//! program cannot read memory layout out of it; only equality is meaningful.
//! The same holds here: the id is a digest of the cell's address under a
//! per-process key, and the reflector keeps its handle on the cell, so an id
//! stays valid (and unique) for as long as anything can compare it.
//!
//! Symfony reaches for this in `VarCloner` and in the deep-clone polyfill, to
//! keep a dumped or cloned structure's shared references shared.

use sha1::{Digest, Sha1};

use rphp_runtime::{nm, ClassFlags, Ctx, NativeResult, Registry, Unwind, Visibility};
use rphp_value::{array_key, Object, PhpRef, Value};

use super::common::{new_reflector, state, this};

/// The cell an instance points at, kept alive so its id cannot be reused.
#[derive(Clone)]
pub(crate) struct RefState {
    cell: PhpRef,
}

/// The per-process key that keeps an id from being an address.
fn key() -> &'static [u8; 16] {
    use std::sync::OnceLock;
    static KEY: OnceLock<[u8; 16]> = OnceLock::new();
    KEY.get_or_init(|| {
        // Any per-process value will do; the point is only that the digest
        // cannot be inverted back to an address.
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| u64::from(d.subsec_nanos()))
            ^ (u64::from(std::process::id()) << 32);
        let stack = &seed as *const u64 as usize as u64;
        let mut k = [0u8; 16];
        k[..8].copy_from_slice(&seed.to_le_bytes());
        k[8..].copy_from_slice(&stack.to_le_bytes());
        k
    })
}

/// `ReflectionReference::fromArrayElement(array $array, int|string $key): ?ReflectionReference`
/// — `null` when the element is not a reference, and php's
/// `ReflectionException` when there is no such element.
fn from_array_element(ctx: &mut Ctx, _this: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let arr = match &*args[0].deref() {
        Value::Array(a) => a.clone(),
        other => {
            return Err(Unwind::type_error(format!(
                "ReflectionReference::fromArrayElement(): Argument #1 ($array) must be of type array, {} given",
                other.type_name()
            )))
        }
    };
    let k = args[1].deref().into_owned();
    let Some(key) = array_key(&k) else {
        return Err(Unwind::type_error(format!(
            "ReflectionReference::fromArrayElement(): Argument #2 ($key) must be of type string|int, {} given",
            k.type_name()
        )));
    };
    let Some(slot) = arr.get(&key) else {
        return Err(Unwind::exception("ReflectionException", "Array key not found"));
    };
    // Only a *reference* element answers; an ordinary one is `null`.
    let Value::Ref(cell) = slot else {
        return Ok(Value::Null);
    };
    let o = new_reflector(ctx, "ReflectionReference", RefState { cell: cell.clone() })?;
    Ok(Value::Object(o))
}

/// `ReflectionReference::getId(): string` — 20 opaque bytes, equal for two
/// elements that share one reference and different for any other pair.
fn get_id(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let s: RefState = state(this(o)?)?;
    let mut h = Sha1::new();
    h.update(key());
    h.update(s.cell.id().to_le_bytes());
    Ok(Value::string(&h.finalize()))
}

/// php's private constructor: the class is only ever built by
/// `fromArrayElement()`.
fn construct(_: &mut Ctx, _: Option<&Object>, _: &mut [Value]) -> NativeResult {
    Ok(Value::Null)
}

/// Register the class.
pub(crate) fn register_classes(r: &mut Registry) {
    r.class("ReflectionReference")
        .flags(ClassFlags::FINAL)
        .method_vis("__construct", Visibility::Private, nm!(0, Some(0), construct))
        .method_vis("__clone", Visibility::Private, nm!(0, Some(0), construct))
        .method("getId", nm!(0, Some(0), get_id))
        .method(
            "fromArrayElement",
            super::common::snm(2, Some(2), from_array_element),
        )
        .finish();
}
