//! The bridge from the generated descriptor tables (`generated/`: php
//! 8.5.10's arginfo, constants and class skeletons for ext/intl) to the
//! runtime's registry: a class is built from its `ClassSig` — every
//! constant, every method with its arity and by-ref mask — and each method
//! is bound to the handler an implementation module provides. A method no
//! module implements yet is bound to [`unimplemented`], which throws an
//! `Error` naming it; `COVERAGE.md` lists those.

use rphp_ext_api::{ClassKind as ApiKind, ClassSig, ConstValue, FnSig, Vis};
use rphp_runtime::{
    ClassBuilder, ClassFlags, ClassKind, Ctx, FnFlags, NativeFn, NativeHandler, NativeMethod, NativeMethodHandler,
    NativeResult, Registry, Unwind, Visibility,
};
use rphp_value::{Object, Value};

/// A method implementation: the method name as php spells it, the handler.
pub type MethodImpl = (&'static str, NativeMethodHandler);

/// A function implementation: the name, the handler.
pub type FnImpl = (&'static str, NativeHandler);

/// The handler of a method no module implements: php's `Error`.
fn unimplemented(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let name = ctx.active_function_name();
    let _ = o;
    Err(Unwind::error(format!(
        "{name}() is not implemented by rphp's intl yet"
    )))
}

/// A `ConstValue` as a runtime value (`Runtime` has no value form; the
/// intl constants have none).
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

fn max_args(sig: &FnSig) -> Option<u8> {
    sig.max_args().map(|n| n.min(255) as u8)
}

/// The class of `sig`, with its constants and methods bound to `impls`
/// (matched by name, case-insensitively); `extra` adds what a skeleton
/// cannot say (payload hooks, iteration).
pub fn register_class(
    r: &mut Registry,
    sig: &'static ClassSig,
    impls: &[MethodImpl],
    extra: impl FnOnce(ClassBuilder<'_>) -> ClassBuilder<'_>,
) -> u32 {
    let mut b = match sig.kind {
        ApiKind::Interface => r.interface(sig.name),
        _ => r.class(sig.name),
    };
    // No intl class is a trait or an enum; the skeleton's kind is class or
    // interface.
    b = match sig.kind {
        ApiKind::Trait => b.kind(ClassKind::Trait),
        _ => b,
    };
    if let Some(parent) = sig.parent {
        b = b.extends(parent);
    }
    if !sig.interfaces.is_empty() {
        b = b.implements(sig.interfaces);
    }
    let mut flags = ClassFlags::NONE;
    if sig.flags.contains(rphp_ext_api::ClassSigFlags::ABSTRACT) {
        flags |= ClassFlags::ABSTRACT;
    }
    if sig.flags.contains(rphp_ext_api::ClassSigFlags::FINAL) {
        flags |= ClassFlags::FINAL;
    }
    b = b.flags(flags);
    for c in sig.consts {
        b = b.class_const(c.name, const_value(&c.value));
    }
    for m in sig.methods {
        let fsig = m.sig;
        let name = fsig.name.rsplit("::").next().unwrap_or(fsig.name);
        let handler = impls
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map_or(unimplemented as NativeMethodHandler, |(_, h)| *h);
        let nm = NativeMethod {
            handler,
            min_args: fsig.required,
            max_args: max_args(fsig),
            params: &[],
            by_ref: by_ref_mask(fsig),
            is_static: m.is_static,
            is_final: m.is_final,
        };
        let vis = match m.vis {
            Vis::Public => Visibility::Public,
            Vis::Protected => Visibility::Protected,
            Vis::Private => Visibility::Private,
        };
        b = if m.is_abstract {
            b.abstract_method(name, nm)
        } else {
            b.method_vis(name, vis, nm)
        };
    }
    b = extra(b);
    b.finish()
}

/// The functions of `sigs` that `impls` implement, as registry rows: the
/// arity and by-ref mask from php's arginfo, the handler from the module.
pub fn register_functions(r: &mut Registry, sigs: &[&'static FnSig], impls: &[FnImpl]) {
    for sig in sigs {
        let Some((_, handler)) = impls.iter().find(|(n, _)| n.eq_ignore_ascii_case(sig.name)) else {
            continue;
        };
        r.function(NativeFn {
            name: sig.name,
            min_args: sig.required,
            max_args: max_args(sig),
            by_ref: by_ref_mask(sig),
            params: &[],
            flags: FnFlags::EMPTY,
            handler: *handler,
        });
    }
}
