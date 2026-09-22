//! The bridge from the generated descriptor tables to the registry: a
//! function is registered with php's own arity and by-ref mask, and only
//! when this crate implements it. A signature with no handler is simply
//! absent, so `function_exists()` tells the truth about what this build
//! answers for.

use rphp_ext_api::{ClassSig, ConstValue, FnSig};
use rphp_runtime::{ClassBuilder, ClassFlags, FnFlags, NativeFn, NativeHandler, Registry};
use rphp_value::Value;

/// A function implementation: the name, the handler.
pub type FnImpl = (&'static str, NativeHandler);

pub fn const_value(v: &ConstValue) -> Value {
    match v {
        ConstValue::Null | ConstValue::Runtime => Value::Null,
        ConstValue::Bool(b) => Value::Bool(*b),
        ConstValue::Int(i) => Value::Int(*i),
        ConstValue::Float(f) => Value::Float(*f),
        ConstValue::Str(s) => Value::string(s.as_bytes()),
    }
}

fn by_ref_mask(sig: &FnSig) -> u32 {
    let mut mask = 0u32;
    for (i, p) in sig.params.iter().enumerate().take(32) {
        if p.by_ref || p.prefer_ref {
            mask |= 1 << i;
        }
    }
    mask
}

pub fn register_functions(r: &mut Registry, sigs: &[&'static FnSig], impls: &[FnImpl]) {
    for sig in sigs {
        let Some((_, handler)) = impls.iter().find(|(n, _)| n.eq_ignore_ascii_case(sig.name)) else {
            continue;
        };
        r.function(NativeFn {
            name: sig.name,
            min_args: sig.required,
            max_args: sig.max_args().map(|n| n.min(255) as u8),
            by_ref: by_ref_mask(sig),
            params: &[],
            flags: FnFlags::EMPTY,
            handler: *handler,
        });
    }
}

/// An opaque handle class: php's `OpenSSLAsymmetricKey` and its neighbours
/// are `final` with no methods and no properties — everything they carry is
/// the native payload.
pub fn register_handle(r: &mut Registry, sig: &'static ClassSig) {
    let b: ClassBuilder<'_> = r.class(sig.name).flags(ClassFlags::FINAL);
    b.finish();
}
