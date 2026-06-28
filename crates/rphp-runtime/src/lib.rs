//! Tier-0 register-bytecode interpreter.
//!
//! A portable `loop { match op { … } }` dispatch over a function's `code`,
//! with a `usize` program counter. Each frame owns a `Vec<Value>` of registers
//! (sized to [`Function::num_regs`], all initialized to `Value::Null` so
//! uninitialized vars read as null). Calls recurse through [`exec_function`],
//! threading a single `&mut RunOutput` so `echo` output accumulates across
//! frames.
//!
//! The `become`-threaded dispatch path is a later refinement behind a feature
//! flag (ADR-001); plain `match` is the correct and required core for M0.
//!
//! Keep the public signatures of [`run`], [`RunOutput`], and [`RuntimeError`]
//! stable; the CLI and tests depend on them.
#![forbid(unsafe_code)]

use rphp_bytecode::{Function, Module, Op};
use rphp_value::{array_key, Str, Value, ValueError};

/// Captured side effects of a run. `echo` output accumulates into `stdout` as
/// raw bytes — PHP strings are byte strings, so the buffer is binary-safe rather
/// than UTF-8.
#[derive(Default, Debug)]
pub struct RunOutput {
    pub stdout: Vec<u8>,
}

/// A runtime fault (division by zero, undefined function, …) surfaced as a
/// PHP-level error.
#[derive(Debug)]
pub struct RuntimeError {
    pub message: String,
}

impl std::fmt::Display for RuntimeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for RuntimeError {}

/// Map a value-level fault to a PHP-flavored runtime error.
fn runtime_error(err: ValueError) -> RuntimeError {
    let message = match err {
        ValueError::DivisionByZero => "Division by zero".to_string(),
        ValueError::ModuloByZero => "Modulo by zero".to_string(),
        ValueError::TypeError(msg) => format!("Unsupported operand types: {msg}"),
    };
    RuntimeError { message }
}

/// Execute `module` starting at its `main` function, returning captured output.
///
/// `main` takes no parameters, so it is entered with an empty argument list and
/// an all-null register frame.
pub fn run(module: &Module) -> Result<RunOutput, RuntimeError> {
    let mut out = RunOutput::default();
    let main = module.func(module.main);
    exec_function(module, main, &[], &mut out)?;
    Ok(out)
}

/// Execute a single function frame to completion, returning its result value.
///
/// `args` holds the values staged by the caller (the callee's registers
/// `0 .. args.len()` are initialized from them, per the calling convention).
///
/// Calls recurse here. Very deep PHP recursion can therefore overflow the host
/// stack; an explicit frame stack is the later (post-M0) design.
fn exec_function(
    module: &Module,
    function: &Function,
    args: &[Value],
    out: &mut RunOutput,
) -> Result<Value, RuntimeError> {
    // The frame: every register starts as null (uninitialized vars read null).
    let mut regs = vec![Value::Null; function.num_regs as usize];
    // Initialize parameter registers `0 .. argc` from the staged arguments.
    for (i, arg) in args.iter().enumerate() {
        regs[i] = arg.clone();
    }

    let mut pc: usize = 0;
    loop {
        // Falling off the end of the code is an implicit `return null`.
        let Some(&op) = function.code.get(pc) else {
            return Ok(Value::Null);
        };

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
                    .map_err(runtime_error)?;
            }
            Op::Sub { dst, a, b } => {
                regs[dst as usize] = regs[a as usize]
                    .sub(&regs[b as usize])
                    .map_err(runtime_error)?;
            }
            Op::Mul { dst, a, b } => {
                regs[dst as usize] = regs[a as usize]
                    .mul(&regs[b as usize])
                    .map_err(runtime_error)?;
            }
            Op::Div { dst, a, b } => {
                regs[dst as usize] = regs[a as usize]
                    .div(&regs[b as usize])
                    .map_err(runtime_error)?;
            }
            Op::Mod { dst, a, b } => {
                regs[dst as usize] = regs[a as usize]
                    .rem(&regs[b as usize])
                    .map_err(runtime_error)?;
            }
            Op::Pow { dst, a, b } => {
                regs[dst as usize] = regs[a as usize]
                    .pow(&regs[b as usize])
                    .map_err(runtime_error)?;
            }
            Op::Neg { dst, src } => {
                regs[dst as usize] = regs[src as usize].neg().map_err(runtime_error)?;
            }

            // --- strings ---
            Op::Concat { dst, a, b } => {
                regs[dst as usize] = regs[a as usize].concat(&regs[b as usize]);
            }

            // --- arrays ---
            Op::NewArray { dst } => {
                regs[dst as usize] = Value::empty_array();
            }
            Op::ArrayGet { dst, base, key } => {
                regs[dst as usize] = array_get(&regs[base as usize], &regs[key as usize]);
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
            Op::ForeachNext { arr, cursor, key_dst, val_dst, target } => {
                let pos = regs[cursor as usize].to_int();
                let entry = match &regs[arr as usize] {
                    Value::Array(a) if pos >= 0 && (pos as usize) < a.len() => {
                        a.entry_at(pos as usize).map(|(k, v)| (k.to_value(), v.clone()))
                    }
                    _ => None,
                };
                match entry {
                    Some((k, v)) => {
                        regs[key_dst as usize] = k;
                        regs[val_dst as usize] = v;
                        regs[cursor as usize] = Value::Int(pos + 1);
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
            Op::Call { dst, func, base, argc } => {
                // Stage `argc` args from the caller window `base ..= base+argc-1`.
                let base = base as usize;
                let mut call_args = Vec::with_capacity(argc as usize);
                for i in 0..argc as usize {
                    call_args.push(regs[base + i].clone());
                }
                let callee = module.func(func);
                let ret = exec_function(module, callee, &call_args, out)?;
                regs[dst as usize] = ret;
            }
            Op::CallNative { dst, native, base, argc } => {
                // Same `base ..= base+argc-1` staging as a user call; the args
                // are handed to the builtin and its result lands in `dst`.
                let base = base as usize;
                let argc = argc as usize;
                let id = rphp_stdlib::NativeId(native);
                let mut call_args: Vec<Value> =
                    (0..argc).map(|i| regs[base + i].clone()).collect();
                let ret = {
                    let mut ctx = rphp_stdlib::Ctx { out: &mut out.stdout };
                    rphp_stdlib::call(id, &mut ctx, &mut call_args)
                        .map_err(|e| RuntimeError { message: e.message })?
                };
                // A by-reference builtin mutates its argument slots in place; copy
                // the window back so the compiler's write-back `Move`s (into the
                // caller's variables) observe the changes. For such calls `dst` is
                // allocated above the window, so it cannot alias a by-ref slot.
                if rphp_stdlib::descriptor(id).by_ref != 0 {
                    for (i, v) in call_args.into_iter().enumerate() {
                        regs[base + i] = v;
                    }
                }
                regs[dst as usize] = ret;
            }
            Op::Ret { src } => {
                return Ok(src.map_or(Value::Null, |r| regs[r as usize].clone()));
            }

            // --- io ---
            Op::Echo { src } => {
                regs[src as usize].append_php_bytes(&mut out.stdout);
            }
        }

        pc += 1;
    }
}

/// `base[key]` read. Arrays index by normalized key (absent ⇒ null); strings
/// index by byte offset (negative allowed; out of range ⇒ "") — both
/// warning-on-miss cases defer the warning. Indexing any other type is null.
fn array_get(base: &Value, key: &Value) -> Value {
    match base {
        Value::Array(a) => match array_key(key) {
            Some(k) => a.get(&k).cloned().unwrap_or(Value::Null),
            None => Value::Null,
        },
        Value::Str(s) => string_offset(s, key),
        _ => Value::Null,
    }
}

fn string_offset(s: &Str, key: &Value) -> Value {
    let len = s.len() as i64;
    let mut i = key.to_int();
    if i < 0 {
        i += len; // PHP allows negative string offsets
    }
    if i >= 0 && i < len {
        Value::string(&s.as_bytes()[i as usize..i as usize + 1])
    } else {
        Value::string(b"") // out of range -> "" (warning deferred)
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

#[cfg(test)]
mod tests {
    use super::*;
    use rphp_bytecode::{Const, Function, Module, Op};
    use rphp_intern::IdentId;
    use rphp_span::Span;

    /// Build a `Function` by hand. `name` only matters for diagnostics, which
    /// the interpreter never inspects, so a fixed id is fine.
    fn func(num_params: u16, num_regs: u16, code: Vec<Op>, consts: Vec<Const>) -> Function {
        Function {
            name: IdentId(0),
            num_params,
            num_regs,
            code,
            consts,
            span: Span::dummy(),
        }
    }

    /// A single-function module whose lone function is `main`.
    fn module(main: Function) -> Module {
        Module { funcs: vec![main], main: 0 }
    }

    /// Run a module and decode its (binary-safe) stdout as UTF-8 for assertions.
    fn out_str(m: &Module) -> String {
        String::from_utf8(run(m).unwrap().stdout).unwrap()
    }

    #[test]
    fn echo_constant() {
        let m = module(func(
            0,
            1,
            vec![
                Op::LoadConst { dst: 0, k: 0 },
                Op::Echo { src: 0 },
                Op::Ret { src: None },
            ],
            vec![Const::Int(42)],
        ));
        assert_eq!(out_str(&m), "42");
    }

    #[test]
    fn add_then_echo() {
        // 1 + 2 => echo "3"
        let m = module(func(
            0,
            3,
            vec![
                Op::LoadConst { dst: 0, k: 0 },
                Op::LoadConst { dst: 1, k: 1 },
                Op::Add { dst: 2, a: 0, b: 1 },
                Op::Echo { src: 2 },
                Op::Ret { src: None },
            ],
            vec![Const::Int(1), Const::Int(2)],
        ));
        assert_eq!(out_str(&m), "3");
    }

    #[test]
    fn jmp_if_false_skips_echo() {
        // cond = false; if (cond) echo "yes"; echo "no"  =>  "no"
        let m = module(func(
            0,
            2,
            vec![
                Op::LoadBool { dst: 0, val: false }, // 0: cond = false
                Op::JmpIfFalse { cond: 0, target: 4 }, // 1: skip the "yes" echo
                Op::LoadConst { dst: 1, k: 0 },      // 2: (skipped)
                Op::Echo { src: 1 },                 // 3: (skipped)
                Op::LoadConst { dst: 1, k: 1 },      // 4: load "no"
                Op::Echo { src: 1 },                 // 5: echo "no"
                Op::Ret { src: None },               // 6
            ],
            vec![Const::Int(111), Const::Int(222)],
        ));
        assert_eq!(out_str(&m), "222");
    }

    #[test]
    fn jmp_if_true_taken() {
        // cond = true; if (cond) echo a; echo b  => "a" only (jump past b's echo)
        let m = module(func(
            0,
            2,
            vec![
                Op::LoadBool { dst: 0, val: true }, // 0
                Op::JmpIfTrue { cond: 0, target: 3 }, // 1 -> echo a
                Op::Jmp { target: 5 },              // 2 -> end (skipped)
                Op::LoadConst { dst: 1, k: 0 },     // 3: a
                Op::Echo { src: 1 },                // 4
                Op::Ret { src: None },              // 5
            ],
            vec![Const::Int(7)],
        ));
        assert_eq!(out_str(&m), "7");
    }

    #[test]
    fn call_returns_value() {
        // main: x = add2(20, 22); echo x   => "42"
        // add2(a, b): return a + b
        let main = func(
            0,
            3,
            vec![
                Op::LoadConst { dst: 0, k: 0 },                       // 0: 20 (arg base)
                Op::LoadConst { dst: 1, k: 1 },                       // 1: 22
                Op::Call { dst: 2, func: 1, base: 0, argc: 2 },       // 2: x = add2(20, 22)
                Op::Echo { src: 2 },                                  // 3
                Op::Ret { src: None },                                // 4
            ],
            vec![Const::Int(20), Const::Int(22)],
        );
        let add2 = func(
            2,
            3,
            vec![
                Op::Add { dst: 2, a: 0, b: 1 }, // 0: a + b
                Op::Ret { src: Some(2) },       // 1: return
            ],
            vec![],
        );
        let m = Module { funcs: vec![main, add2], main: 0 };
        assert_eq!(out_str(&m), "42");
    }

    #[test]
    fn recursive_call_factorial() {
        // fact(n): if (n <= 1) return 1; return n * fact(n - 1)
        // main: echo fact(5)  => "120"
        let main = func(
            0,
            2,
            vec![
                Op::LoadConst { dst: 0, k: 0 },                 // 0: 5
                Op::Call { dst: 1, func: 1, base: 0, argc: 1 }, // 1: fact(5)
                Op::Echo { src: 1 },                            // 2
                Op::Ret { src: None },                          // 3
            ],
            vec![Const::Int(5)],
        );
        // regs: 0 = n (param), 1 = one, 2 = cond, 3 = n-1, 4 = recurse result, 5 = product
        let fact = func(
            1,
            6,
            vec![
                Op::LoadConst { dst: 1, k: 0 },                 // 0: one = 1
                Op::CmpLe { dst: 2, a: 0, b: 1 },               // 1: cond = n <= 1
                Op::JmpIfFalse { cond: 2, target: 4 },          // 2: if !cond -> recurse
                Op::Ret { src: Some(1) },                       // 3: return 1
                Op::Sub { dst: 3, a: 0, b: 1 },                 // 4: n - 1
                Op::Call { dst: 4, func: 1, base: 3, argc: 1 }, // 5: fact(n-1)
                Op::Mul { dst: 5, a: 0, b: 4 },                 // 6: n * result
                Op::Ret { src: Some(5) },                       // 7
            ],
            vec![Const::Int(1)],
        );
        let m = Module { funcs: vec![main, fact], main: 0 };
        assert_eq!(out_str(&m), "120");
    }

    #[test]
    fn division_by_zero_errors() {
        let m = module(func(
            0,
            3,
            vec![
                Op::LoadConst { dst: 0, k: 0 },
                Op::LoadConst { dst: 1, k: 1 },
                Op::Div { dst: 2, a: 0, b: 1 },
                Op::Echo { src: 2 },
                Op::Ret { src: None },
            ],
            vec![Const::Int(1), Const::Int(0)],
        ));
        let err = run(&m).unwrap_err();
        assert_eq!(err.message, "Division by zero");
    }

    #[test]
    fn modulo_by_zero_errors() {
        let m = module(func(
            0,
            3,
            vec![
                Op::LoadConst { dst: 0, k: 0 },
                Op::LoadConst { dst: 1, k: 1 },
                Op::Mod { dst: 2, a: 0, b: 1 },
                Op::Ret { src: None },
            ],
            vec![Const::Int(7), Const::Int(0)],
        ));
        assert_eq!(run(&m).unwrap_err().message, "Modulo by zero");
    }

    #[test]
    fn uninitialized_register_reads_null() {
        // Echo a never-written register => null prints as the empty string.
        let m = module(func(
            0,
            1,
            vec![Op::Echo { src: 0 }, Op::Ret { src: None }],
            vec![],
        ));
        assert_eq!(out_str(&m), "");
    }

    #[test]
    fn comparison_and_float_div() {
        // echo (7 / 2)  => "3.5"  ; then echo (3 <=> 5) => "-1"
        let m = module(func(
            0,
            3,
            vec![
                Op::LoadConst { dst: 0, k: 0 },        // 7
                Op::LoadConst { dst: 1, k: 1 },        // 2
                Op::Div { dst: 2, a: 0, b: 1 },        // 3.5
                Op::Echo { src: 2 },
                Op::LoadConst { dst: 0, k: 2 },        // 3
                Op::LoadConst { dst: 1, k: 3 },        // 5
                Op::Spaceship { dst: 2, a: 0, b: 1 },  // -1
                Op::Echo { src: 2 },
                Op::Ret { src: None },
            ],
            vec![Const::Int(7), Const::Int(2), Const::Int(3), Const::Int(5)],
        ));
        assert_eq!(out_str(&m), "3.5-1");
    }

    #[test]
    fn loop_with_backward_jump() {
        // sum = 0; i = 3; while (i > 0) { sum += i; i -= 1; } echo sum  => "6"
        let m = module(func(
            0,
            5,
            vec![
                Op::LoadConst { dst: 0, k: 0 },        // 0: sum = 0
                Op::LoadConst { dst: 1, k: 1 },        // 1: i = 3
                Op::LoadConst { dst: 2, k: 0 },        // 2: zero = 0
                Op::LoadConst { dst: 3, k: 2 },        // 3: one = 1
                Op::CmpGt { dst: 4, a: 1, b: 2 },      // 4: head: cond = i > 0
                Op::JmpIfFalse { cond: 4, target: 9 }, // 5: exit -> pc 9
                Op::Add { dst: 0, a: 0, b: 1 },        // 6: sum += i
                Op::Sub { dst: 1, a: 1, b: 3 },        // 7: i -= 1
                Op::Jmp { target: 4 },                 // 8: back to head
                Op::Echo { src: 0 },                   // 9: echo sum
                Op::Ret { src: None },                 // 10
            ],
            vec![Const::Int(0), Const::Int(3), Const::Int(1)],
        ));
        assert_eq!(out_str(&m), "6");
    }

    #[test]
    fn concat_and_echo_string() {
        use rphp_value::Str;
        // echo "Hi, " . "PHP" . "!\n";  =>  "Hi, PHP!\n"
        let m = module(func(
            0,
            3,
            vec![
                Op::LoadConst { dst: 0, k: 0 },   // "Hi, "
                Op::LoadConst { dst: 1, k: 1 },   // "PHP"
                Op::Concat { dst: 0, a: 0, b: 1 },
                Op::LoadConst { dst: 1, k: 2 },   // "!\n"
                Op::Concat { dst: 0, a: 0, b: 1 },
                Op::Echo { src: 0 },
                Op::Ret { src: None },
            ],
            vec![
                Const::Str(Str::new(b"Hi, ")),
                Const::Str(Str::new(b"PHP")),
                Const::Str(Str::new(b"!\n")),
            ],
        ));
        assert_eq!(out_str(&m), "Hi, PHP!\n");
    }

    #[test]
    fn array_build_index_and_get() {
        // $a = []; $a[] = 10; $a[] = 20; echo $a[1];  => "20"
        let m = module(func(
            0,
            3,
            vec![
                Op::NewArray { dst: 0 },                       // $a
                Op::LoadConst { dst: 1, k: 0 },                // 10
                Op::ArrayPush { arr: 0, value: 1 },
                Op::LoadConst { dst: 1, k: 1 },                // 20
                Op::ArrayPush { arr: 0, value: 1 },
                Op::LoadConst { dst: 1, k: 2 },                // index 1
                Op::ArrayGet { dst: 2, base: 0, key: 1 },
                Op::Echo { src: 2 },
                Op::Ret { src: None },
            ],
            vec![Const::Int(10), Const::Int(20), Const::Int(1)],
        ));
        assert_eq!(out_str(&m), "20");
    }

    #[test]
    fn foreach_sums_values() {
        // $a = [3, 4, 5]; foreach ($a as $v) { $s = $s + $v; } echo $s;  => "12"
        // regs: 0=$a, 1=$s, 2=$v, 3=arr-snapshot, 4=cursor, 5=tmp
        let m = module(func(
            0,
            6,
            vec![
                Op::NewArray { dst: 0 },                 // 0: $a = []
                Op::LoadConst { dst: 5, k: 0 },          // 1: 3
                Op::ArrayPush { arr: 0, value: 5 },      // 2
                Op::LoadConst { dst: 5, k: 1 },          // 3: 4
                Op::ArrayPush { arr: 0, value: 5 },      // 4
                Op::LoadConst { dst: 5, k: 2 },          // 5: 5
                Op::ArrayPush { arr: 0, value: 5 },      // 6
                Op::LoadConst { dst: 1, k: 3 },          // 7: $s = 0
                Op::Move { dst: 3, src: 0 },             // 8: snapshot
                Op::LoadConst { dst: 4, k: 3 },          // 9: cursor = 0
                Op::ForeachNext { arr: 3, cursor: 4, key_dst: 5, val_dst: 2, target: 13 }, // 10
                Op::Add { dst: 1, a: 1, b: 2 },          // 11: $s += $v
                Op::Jmp { target: 10 },                  // 12
                Op::Echo { src: 1 },                     // 13
                Op::Ret { src: None },                   // 14
            ],
            vec![Const::Int(3), Const::Int(4), Const::Int(5), Const::Int(0)],
        ));
        assert_eq!(out_str(&m), "12");
    }

    #[test]
    fn echo_preserves_raw_bytes() {
        use rphp_value::Str;
        // A non-UTF-8 byte (0xFF) must survive echo unchanged.
        let m = module(func(
            0,
            1,
            vec![Op::LoadConst { dst: 0, k: 0 }, Op::Echo { src: 0 }, Op::Ret { src: None }],
            vec![Const::Str(Str::new(&[0xFF, 0x00, 0x41]))],
        ));
        assert_eq!(run(&m).unwrap().stdout, vec![0xFF, 0x00, 0x41]);
    }
}
