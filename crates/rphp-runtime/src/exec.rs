//! Tier-0 register-bytecode interpreter.
//!
//! A portable `loop { match op { … } }` dispatch over a function's `code`,
//! with a `usize` program counter. Each frame owns a `Vec<Value>` of registers
//! (sized to `Function::num_regs`, all initialized to `Value::Null` so
//! uninitialized vars read as null). Calls recurse through [`exec_function`],
//! threading the one [`Interp`]. Every fault is an [`Unwind`] (ADR-022): the
//! value layer's `ValueError`s become `DivisionByZeroError` / `TypeError`
//! pending throws, engine faults (`Call to undefined method X::m()`, …)
//! `Error`s, and the cheap VM warnings (undefined array key / property /
//! offset, array-to-string) go through the diagnostics channel.
//!
//! The recursive `exec_function` shape is replaced by E3's explicit frame
//! stack; the frame-info stack in [`Interp`] gives line numbers and traces
//! until then.

use rphp_bytecode::{ClassId, FuncId, Function, Module, Op, Visibility};
use rphp_value::{array_key, ArrayKey, Object, Str, Value, ValueError};

use crate::frames::FrameInfo;
use crate::registry::{NativeId, Unwind};
use crate::Interp;

/// Map a value-level fault to its PHP throwable.
fn value_fault(err: ValueError) -> Unwind {
    match err {
        ValueError::DivisionByZero => Unwind::division_by_zero("Division by zero"),
        ValueError::ModuloByZero => Unwind::division_by_zero("Modulo by zero"),
        ValueError::TypeError(msg) => {
            Unwind::type_error(format!("Unsupported operand types: {msg}"))
        }
    }
}

/// php's `zend_zval_value_name`: the name a value has in messages such as
/// `Call to a member function m() on int` / `… on true`.
pub fn value_name(v: &Value) -> String {
    match &*v.deref() {
        Value::Null | Value::Uninit => "null".to_string(),
        Value::Bool(true) => "true".to_string(),
        Value::Bool(false) => "false".to_string(),
        Value::Int(_) => "int".to_string(),
        Value::Float(_) => "float".to_string(),
        Value::Str(_) => "string".to_string(),
        Value::Array(_) => "array".to_string(),
        Value::Closure(_) => "Closure".to_string(),
        Value::Object(o) => String::from_utf8_lossy(o.layout().class_name()).into_owned(),
        Value::Resource(_) => "resource".to_string(),
        Value::Ref(_) => unreachable!("deref'd above"),
    }
}

/// Whether a member with the given visibility, declared in `decl_class`, is
/// reachable from code executing in `cur_class`. `protected` is visible anywhere
/// in the same inheritance hierarchy; `private` only within the declaring class.
fn access_ok(
    module: &Module,
    vis: Visibility,
    decl_class: ClassId,
    cur_class: Option<ClassId>,
) -> bool {
    match vis {
        Visibility::Public => true,
        Visibility::Private => cur_class == Some(decl_class),
        Visibility::Protected => match cur_class {
            Some(cc) => {
                module.is_subclass_or_eq(cc, decl_class) || module.is_subclass_or_eq(decl_class, cc)
            }
            None => false,
        },
    }
}

fn vis_word(v: Visibility) -> &'static str {
    match v {
        Visibility::Public => "public",
        Visibility::Protected => "protected",
        Visibility::Private => "private",
    }
}

/// Enforce property visibility. An undeclared (dynamic) property is public, so a
/// `None` resolution is always allowed.
fn check_prop_access(
    module: &Module,
    class: ClassId,
    name: &[u8],
    cur_class: Option<ClassId>,
) -> Result<(), Unwind> {
    if let Some((vis, decl)) = module.resolve_prop(class, name) {
        if !access_ok(module, vis, decl, cur_class) {
            return Err(Unwind::error(format!(
                "Cannot access {} property {}::${}",
                vis_word(vis),
                String::from_utf8_lossy(&module.class(decl).name_bytes),
                String::from_utf8_lossy(name),
            )));
        }
    }
    Ok(())
}

/// Invoke a callable value with `args`: a closure, or a function-name string
/// resolving to a user function (declared params only — extra args ignored, as in
/// PHP) or a native. The single path shared by the `CallDynamic` opcode and
/// [`Interp::call_value`].
pub(crate) fn invoke_value(
    interp: &mut Interp,
    module: &Module,
    callee: &Value,
    args: &[Value],
) -> Result<Value, Unwind> {
    let callee = callee.deref();
    match &*callee {
        Value::Closure(c) => return exec_closure(interp, module, c, args),
        Value::Array(_) => {
            return Err(Unwind::error(
                "Array callback must have exactly two elements",
            ))
        }
        Value::Object(o) => {
            return Err(Unwind::error(format!(
                "Object of type {} is not callable",
                String::from_utf8_lossy(o.layout().class_name())
            )))
        }
        _ => {}
    }
    let name = callee.to_php_bytes();
    if !name.is_empty() {
        if let Some(fid) = module.func_by_name(&name) {
            let f = module.func(fid);
            let n = (f.num_params as usize).min(args.len());
            // A callable string resolves to a free function — no class context.
            return exec_function(interp, module, fid, &args[..n], None);
        }
        if let Some(nid) = interp.native_by_name(&name) {
            let mut args = args.to_vec();
            return interp.call_native(nid, &mut args);
        }
    }
    Err(Unwind::error(format!(
        "Call to undefined function {}()",
        String::from_utf8_lossy(&name)
    )))
}

/// Execute a single function frame to completion, returning its result value.
///
/// `args` holds the values staged by the caller (the callee's registers
/// `0 .. args.len()` are initialized from them, per the calling convention).
///
/// Calls recurse here. Very deep PHP recursion can therefore overflow the host
/// stack; an explicit frame stack is the later (E3) design.
pub(crate) fn exec_function(
    interp: &mut Interp,
    module: &Module,
    fid: FuncId,
    args: &[Value],
    cur_class: Option<ClassId>,
) -> Result<Value, Unwind> {
    let function = module.func(fid);
    // The frame: every register starts as null (uninitialized vars read null).
    let mut regs = vec![Value::Null; function.num_regs as usize];
    // Initialize parameter registers `0 .. argc` from the staged arguments.
    for (i, arg) in args.iter().enumerate().take(regs.len()) {
        regs[i] = arg.clone();
    }
    interp.push_frame(FrameInfo::user(fid, args.to_vec()));
    let r = run_frame(interp, module, function, regs, cur_class);
    let r = interp.locate_fault(r);
    interp.pop_frame();
    r
}

/// Execute a closure: seed the captured environment into the function's
/// `capture_regs`, then bind the parameters (capped to the declared count, so an
/// extra callback argument is ignored as in PHP), then run.
fn exec_closure(
    interp: &mut Interp,
    module: &Module,
    closure: &rphp_value::Closure,
    args: &[Value],
) -> Result<Value, Unwind> {
    let fid = closure.func();
    let function = module.func(fid);
    let mut regs = vec![Value::Null; function.num_regs as usize];
    for (i, &reg) in function.capture_regs.iter().enumerate() {
        regs[reg as usize] = closure.captures()[i].clone();
    }
    let np = function.num_params as usize;
    for (i, arg) in args.iter().enumerate().take(np.min(regs.len())) {
        regs[i] = arg.clone();
    }
    interp.push_frame(FrameInfo::user(
        fid,
        args.iter().take(np).cloned().collect(),
    ));
    // A closure carries no class context (its `$this`/visibility binding is a
    // later refinement).
    let r = run_frame(interp, module, function, regs, None);
    let r = interp.locate_fault(r);
    interp.pop_frame();
    r
}

/// Run a prepared frame (registers already seeded) to completion. `cur_class` is
/// the class whose method is executing (the lexical context for visibility
/// checks), or `None` for free functions, closures, and `{main}`.
fn run_frame(
    interp: &mut Interp,
    module: &Module,
    function: &Function,
    mut regs: Vec<Value>,
    cur_class: Option<ClassId>,
) -> Result<Value, Unwind> {
    let mut pc: usize = 0;
    loop {
        // Falling off the end of the code is an implicit `return null`.
        let Some(&op) = function.code.get(pc) else {
            return Ok(Value::Null);
        };
        interp.set_pc(pc);

        match op {
            // --- moves / constants ---
            Op::LoadConst { dst, k } => {
                regs[dst as usize] = function.consts[k as usize].to_value();
            }
            Op::LoadNull { dst } => {
                regs[dst as usize] = Value::Null;
            }
            Op::LoadBool { dst, val } => {
                regs[dst as usize] = Value::Bool(val);
            }
            Op::Move { dst, src } => {
                regs[dst as usize] = regs[src as usize].clone();
            }

            // --- arithmetic (dst = a OP b) ---
            Op::Add { dst, a, b } => {
                regs[dst as usize] = regs[a as usize]
                    .add(&regs[b as usize])
                    .map_err(value_fault)?;
            }
            Op::Sub { dst, a, b } => {
                regs[dst as usize] = regs[a as usize]
                    .sub(&regs[b as usize])
                    .map_err(value_fault)?;
            }
            Op::Mul { dst, a, b } => {
                regs[dst as usize] = regs[a as usize]
                    .mul(&regs[b as usize])
                    .map_err(value_fault)?;
            }
            Op::Div { dst, a, b } => {
                regs[dst as usize] = regs[a as usize]
                    .div(&regs[b as usize])
                    .map_err(value_fault)?;
            }
            Op::Mod { dst, a, b } => {
                regs[dst as usize] = regs[a as usize]
                    .rem(&regs[b as usize])
                    .map_err(value_fault)?;
            }
            Op::Pow { dst, a, b } => {
                regs[dst as usize] = regs[a as usize]
                    .pow(&regs[b as usize])
                    .map_err(value_fault)?;
            }
            Op::Neg { dst, src } => {
                regs[dst as usize] = regs[src as usize].neg().map_err(value_fault)?;
            }

            // --- strings ---
            Op::Concat { dst, a, b } => {
                if is_array(&regs[a as usize]) {
                    interp.warn("Array to string conversion")?;
                }
                if is_array(&regs[b as usize]) {
                    interp.warn("Array to string conversion")?;
                }
                regs[dst as usize] = regs[a as usize].concat(&regs[b as usize]);
            }

            // --- arrays ---
            Op::NewArray { dst } => {
                regs[dst as usize] = Value::empty_array();
            }
            Op::ArrayGet { dst, base, key } => {
                regs[dst as usize] = array_get(interp, &regs[base as usize], &regs[key as usize])?;
            }
            Op::ArraySet { arr, key, value } => {
                let key = regs[key as usize].clone();
                let value = regs[value as usize].clone();
                array_set(&mut regs[arr as usize], &key, value);
            }
            Op::ArrayPush { arr, value } => {
                let value = regs[value as usize].clone();
                let slot = &mut regs[arr as usize];
                if matches!(slot, Value::Null) {
                    *slot = Value::empty_array();
                }
                if let Value::Array(a) = slot {
                    a.push(value);
                }
            }
            Op::ForeachNext {
                arr,
                cursor,
                key_dst,
                val_dst,
                target,
            } => {
                // The cursor is a *raw* entry position (tombstones included);
                // `next_live_from` skips holes left by `unset`, and the value is
                // dereferenced (a by-value `foreach` sees plain values).
                let pos = regs[cursor as usize].to_int().max(0) as usize;
                let entry = match &*regs[arr as usize].deref() {
                    Value::Array(a) => a
                        .next_live_from(pos)
                        .map(|(raw, k, v)| (raw, k.to_value(), v.deref().into_owned())),
                    _ => None,
                };
                match entry {
                    Some((raw, k, v)) => {
                        regs[key_dst as usize] = k;
                        regs[val_dst as usize] = v;
                        regs[cursor as usize] = Value::Int(raw as i64 + 1);
                    }
                    None => {
                        pc = target as usize;
                        continue;
                    }
                }
            }

            // --- comparison (dst = bool) ---
            Op::CmpEq { dst, a, b } => {
                regs[dst as usize] = Value::Bool(regs[a as usize].loose_eq(&regs[b as usize]));
            }
            Op::CmpNe { dst, a, b } => {
                regs[dst as usize] = Value::Bool(!regs[a as usize].loose_eq(&regs[b as usize]));
            }
            Op::CmpIdentical { dst, a, b } => {
                regs[dst as usize] = Value::Bool(regs[a as usize].identical(&regs[b as usize]));
            }
            Op::CmpNotIdentical { dst, a, b } => {
                regs[dst as usize] = Value::Bool(!regs[a as usize].identical(&regs[b as usize]));
            }
            Op::CmpLt { dst, a, b } => {
                regs[dst as usize] = Value::Bool(regs[a as usize].lt(&regs[b as usize]));
            }
            Op::CmpLe { dst, a, b } => {
                regs[dst as usize] = Value::Bool(regs[a as usize].le(&regs[b as usize]));
            }
            Op::CmpGt { dst, a, b } => {
                regs[dst as usize] = Value::Bool(regs[a as usize].gt(&regs[b as usize]));
            }
            Op::CmpGe { dst, a, b } => {
                regs[dst as usize] = Value::Bool(regs[a as usize].ge(&regs[b as usize]));
            }
            Op::Spaceship { dst, a, b } => {
                regs[dst as usize] = Value::Int(regs[a as usize].spaceship(&regs[b as usize]));
            }
            Op::Not { dst, src } => {
                regs[dst as usize] = regs[src as usize].not();
            }

            // --- control flow ---
            Op::Jmp { target } => {
                pc = target as usize;
                continue;
            }
            Op::JmpIfTrue { cond, target } => {
                if regs[cond as usize].to_bool() {
                    pc = target as usize;
                    continue;
                }
            }
            Op::JmpIfFalse { cond, target } => {
                if !regs[cond as usize].to_bool() {
                    pc = target as usize;
                    continue;
                }
            }

            // --- calls ---
            Op::Call {
                dst,
                func,
                base,
                argc,
            } => {
                // Stage `argc` args from the caller window `base ..= base+argc-1`.
                let base = base as usize;
                let call_args: Vec<Value> =
                    (0..argc as usize).map(|i| regs[base + i].clone()).collect();
                let ret = exec_function(interp, module, func, &call_args, None)?;
                regs[dst as usize] = ret;
            }
            Op::CallNative {
                dst,
                native,
                base,
                argc,
            } => {
                // Same `base ..= base+argc-1` staging as a user call; the args
                // are handed to the native and its result lands in `dst`.
                let base = base as usize;
                let id = NativeId(native);
                let mut call_args: Vec<Value> =
                    (0..argc as usize).map(|i| regs[base + i].clone()).collect();
                let ret = interp.call_native(id, &mut call_args)?;
                // A by-reference native mutates its argument slots in place; copy
                // the window back so the compiler's write-back `Move`s (into the
                // caller's variables) observe the changes. For such calls `dst` is
                // allocated above the window, so it cannot alias a by-ref slot.
                if interp.native(id).by_ref != 0 {
                    for (i, v) in call_args.into_iter().enumerate() {
                        regs[base + i] = v;
                    }
                }
                regs[dst as usize] = ret;
            }
            Op::MakeClosure { dst, proto } => {
                // Snapshot the captured registers and bind them to the closure's
                // compiled function (the template lives in this function).
                let proto = &function.closures[proto as usize];
                let captures: Vec<Value> = proto
                    .src_regs
                    .iter()
                    .map(|&r| regs[r as usize].clone())
                    .collect();
                regs[dst as usize] = Value::Closure(rphp_value::Closure::new(proto.func, captures));
            }
            Op::CallDynamic {
                dst,
                callee,
                base,
                argc,
            } => {
                let base = base as usize;
                let call_args: Vec<Value> =
                    (0..argc as usize).map(|i| regs[base + i].clone()).collect();
                let callee_val = regs[callee as usize].clone();
                regs[dst as usize] = invoke_value(interp, module, &callee_val, &call_args)?;
            }

            // --- objects ---
            Op::New { dst, class } => {
                // Seed the instance from the class's cached layout (its full
                // inherited + own property set) and give it the next handle id.
                let (layout, defaults) = interp.instance_layout(module, class);
                let id = interp.object_ids.alloc();
                regs[dst as usize] = Value::Object(Object::new(class, id, layout, defaults));
            }
            Op::PropGet { dst, obj, name } => {
                let key = function.consts[name as usize].to_value().to_php_bytes();
                let val = match &regs[obj as usize] {
                    Value::Object(o) => {
                        check_prop_access(module, o.class_id(), &key, cur_class)?;
                        match o.get_deref(&key) {
                            Some(v) => v,
                            None => {
                                let msg = format!(
                                    "Undefined property: {}::${}",
                                    String::from_utf8_lossy(o.layout().class_name()),
                                    String::from_utf8_lossy(&key)
                                );
                                interp.warn(&msg)?;
                                Value::Null
                            }
                        }
                    }
                    other => {
                        let msg = format!(
                            "Attempt to read property \"{}\" on {}",
                            String::from_utf8_lossy(&key),
                            value_name(other)
                        );
                        interp.warn(&msg)?;
                        Value::Null
                    }
                };
                regs[dst as usize] = val;
            }
            Op::PropSet { obj, name, value } => {
                let key = function.consts[name as usize].to_value().to_php_bytes();
                let v = regs[value as usize].clone();
                match &regs[obj as usize] {
                    Value::Object(o) => {
                        check_prop_access(module, o.class_id(), &key, cur_class)?;
                        o.set(&key, v);
                    }
                    other => {
                        return Err(Unwind::error(format!(
                            "Attempt to assign property \"{}\" on {}",
                            String::from_utf8_lossy(&key),
                            value_name(other)
                        )));
                    }
                }
            }
            Op::MethodCall {
                dst,
                obj,
                method,
                base,
                argc,
            } => {
                let mname = function.consts[method as usize].to_value().to_php_bytes();
                let obj_val = regs[obj as usize].clone();
                let class_id = match &obj_val {
                    Value::Object(o) => o.class_id(),
                    other => {
                        return Err(Unwind::error(format!(
                            "Call to a member function {}() on {}",
                            String::from_utf8_lossy(&mname),
                            value_name(other),
                        )))
                    }
                };
                // Virtual dispatch: resolve up the chain from the runtime class.
                let (fid, vis, decl_class) =
                    module.resolve_method(class_id, &mname).ok_or_else(|| {
                        Unwind::error(format!(
                            "Call to undefined method {}::{}()",
                            String::from_utf8_lossy(&module.class(class_id).name_bytes),
                            String::from_utf8_lossy(&mname),
                        ))
                    })?;
                if !access_ok(module, vis, decl_class, cur_class) {
                    return Err(Unwind::error(format!(
                        "Call to {} method {}::{}() from {}",
                        vis_word(vis),
                        String::from_utf8_lossy(&module.class(decl_class).name_bytes),
                        String::from_utf8_lossy(&mname),
                        match cur_class {
                            Some(c) => format!(
                                "scope {}",
                                String::from_utf8_lossy(&module.class(c).name_bytes)
                            ),
                            None => "global scope".to_string(),
                        }
                    )));
                }
                let callee = module.func(fid);
                // The method frame takes `$this` in register 0, then its declared
                // parameters; cap the staged args to that count (extra args are
                // ignored, missing ones default to null) so the frame never reads
                // out of bounds.
                let np = callee.num_params as usize;
                let base = base as usize;
                let mut call_args = Vec::with_capacity(np);
                call_args.push(obj_val);
                for i in 0..(argc as usize).min(np.saturating_sub(1)) {
                    call_args.push(regs[base + i].clone());
                }
                // The callee runs in the lexical context of its *declaring* class.
                regs[dst as usize] =
                    exec_function(interp, module, fid, &call_args, Some(decl_class))?;
            }
            Op::StaticCall {
                dst,
                this,
                func,
                base,
                argc,
            } => {
                // Non-virtual scoped call (`self::`/`parent::`/`Class::`); the
                // current `$this` is forwarded explicitly.
                let this_val = regs[this as usize].clone();
                let callee = module.func(func);
                let np = callee.num_params as usize;
                let base = base as usize;
                let mut call_args = Vec::with_capacity(np);
                call_args.push(this_val);
                for i in 0..(argc as usize).min(np.saturating_sub(1)) {
                    call_args.push(regs[base + i].clone());
                }
                let callee_class = module.method_owner(func);
                regs[dst as usize] = exec_function(interp, module, func, &call_args, callee_class)?;
            }
            Op::InstanceOf { dst, obj, class } => {
                let result = match &regs[obj as usize] {
                    Value::Object(o) => module.is_subclass_or_eq(o.class_id(), class),
                    _ => false,
                };
                regs[dst as usize] = Value::Bool(result);
            }

            Op::Ret { src } => {
                return Ok(src.map_or(Value::Null, |r| regs[r as usize].clone()));
            }

            // --- io ---
            Op::Echo { src } => {
                if is_array(&regs[src as usize]) {
                    interp.warn("Array to string conversion")?;
                }
                interp.echo_value(&regs[src as usize]);
            }

            // The v2 contract ops (plan Track E) are not executed by the tier-0
            // interpreter yet; the compiler never emits them either.
            _ => {
                return Err(Unwind::error(format!(
                    "internal error: opcode not implemented by the tier-0 interpreter: {op:?}"
                )));
            }
        }

        pc += 1;
    }
}

fn is_array(v: &Value) -> bool {
    matches!(&*v.deref(), Value::Array(_))
}

/// `base[key]` read. Arrays index by normalized key (absent ⇒ warning + null);
/// strings index by byte offset (negative allowed; out of range ⇒ warning +
/// ""). Indexing a scalar / null warns and yields null.
fn array_get(interp: &mut Interp, base: &Value, key: &Value) -> Result<Value, Unwind> {
    let base = base.deref();
    match &*base {
        Value::Array(a) => match array_key(key) {
            // The element is dereferenced: a read never yields a `Ref`.
            Some(k) => match a.get_deref(&k) {
                Some(v) => Ok(v),
                None => {
                    let msg = match &k {
                        ArrayKey::Int(i) => format!("Undefined array key {i}"),
                        ArrayKey::Str(s) => {
                            format!("Undefined array key \"{}\"", String::from_utf8_lossy(s))
                        }
                    };
                    interp.warn(&msg)?;
                    Ok(Value::Null)
                }
            },
            // Illegal offset type (array/object key): TypeError later (E5).
            None => Ok(Value::Null),
        },
        Value::Str(s) => string_offset(interp, s, key),
        Value::Null | Value::Uninit | Value::Bool(_) | Value::Int(_) | Value::Float(_) => {
            let msg = format!("Trying to access array offset on {}", value_name(&base));
            interp.warn(&msg)?;
            Ok(Value::Null)
        }
        // Objects (ArrayAccess) and resources: later.
        _ => Ok(Value::Null),
    }
}

fn string_offset(interp: &mut Interp, s: &Str, key: &Value) -> Result<Value, Unwind> {
    let len = s.len() as i64;
    let requested = key.to_int();
    let mut i = requested;
    if i < 0 {
        i += len; // PHP allows negative string offsets
    }
    if i >= 0 && i < len {
        Ok(Value::string(&s.as_bytes()[i as usize..i as usize + 1]))
    } else {
        interp.warn(&format!("Uninitialized string offset {requested}"))?;
        Ok(Value::string(b""))
    }
}

/// `slot[key] = value`, mutating in place. Null auto-vivifies to a fresh array;
/// an illegal offset type or a scalar base is a no-op (warning deferred). The
/// COW separation happens inside [`rphp_value::Array::set`].
fn array_set(slot: &mut Value, key: &Value, value: Value) {
    if matches!(slot, Value::Null) {
        *slot = Value::empty_array();
    }
    if let Value::Array(a) = slot {
        if let Some(k) = array_key(key) {
            a.set(k, value);
        }
    }
}
