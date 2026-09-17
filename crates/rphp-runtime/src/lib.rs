//! The engine crate (ADR-014): [`Interp`] owns all per-request state, natives
//! are registered through [`Registry`] and receive [`Ctx`], and every runtime
//! path returns `Result<_, Unwind>` (ADR-022). The stdlib and extension crates
//! sit *above* this crate; the SAPIs reach it through `rphp-embed`.
//!
//! Modules:
//! * `registry` — the native ABI: [`NativeFn`], [`Ctx`], [`Unwind`], [`Registry`].
//! * `interp` — [`Interp`] and the request lifecycle (`run_main`, shutdown
//!   functions, output flush).
//! * `api` — the helper surface natives call (`out`, `warn`, `call_value`,
//!   `ini_*`, constants, resources, `ob_*`).
//! * `errors` — the diagnostics channel (`emit_error`, uncaught rendering).
//! * `output` — the `ob_*` stack over a streaming [`OutputSink`].
//! * `ini`, `resources`, `frames` — the ini table, the resource table, and the
//!   frame-info stack behind line numbers and `Stack trace:`.
//! * `exec` — the tier-0 interpreter loop (replaced by E3).
#![forbid(unsafe_code)]

mod api;
mod errors;
mod exec;
mod frames;
mod ini;
mod interp;
mod output;
mod registry;
mod resources;

pub use api::parse_error_reporting;
pub use errors::{
    DisplayMode, ErrLevel, LastError, E_ALL, E_COMPILE_ERROR, E_COMPILE_WARNING, E_CORE_ERROR,
    E_CORE_WARNING, E_DEPRECATED, E_ERROR, E_NOTICE, E_PARSE, E_RECOVERABLE_ERROR, E_STRICT,
    E_USER_DEPRECATED, E_USER_ERROR, E_USER_NOTICE, E_USER_WARNING, E_WARNING, SILENCE_MASK,
};
pub use exec::value_name;
pub use frames::{frame_name, render_trace, trace_arg, FrameInfo, FrameKind};
pub use ini::{parse_bool, IniEntry, IniTable, CORE_DEFAULTS};
pub use interp::{ExtState, Interp, SapiKind};
pub use output::{
    NullSink, ObLevel, OutputSink, OutputStack, SharedBuffer, PHP_OUTPUT_HANDLER_CLEAN,
    PHP_OUTPUT_HANDLER_CLEANABLE, PHP_OUTPUT_HANDLER_DISABLED, PHP_OUTPUT_HANDLER_FINAL,
    PHP_OUTPUT_HANDLER_FLUSH, PHP_OUTPUT_HANDLER_FLUSHABLE, PHP_OUTPUT_HANDLER_PROCESSED,
    PHP_OUTPUT_HANDLER_REMOVABLE, PHP_OUTPUT_HANDLER_START, PHP_OUTPUT_HANDLER_STARTED,
    PHP_OUTPUT_HANDLER_STDFLAGS, PHP_OUTPUT_HANDLER_USER,
};
pub use registry::{
    Ctx, ErrorKind, FaultSite, FnFlags, NativeFn, NativeHandler, NativeId, NativeResult,
    PendingThrow, Registry, Unwind,
};
pub use resources::ResourceTable;

#[cfg(test)]
mod tests {
    use super::*;
    use rphp_bytecode::{Const, Function, Module, Op};
    use rphp_intern::IdentId;
    use rphp_span::Span;
    use rphp_value::Value;

    /// Build a `Function` by hand. `name` only matters for diagnostics, which
    /// the interpreter never inspects, so a fixed id is fine.
    fn func(num_params: u16, num_regs: u16, code: Vec<Op>, consts: Vec<Const>) -> Function {
        Function {
            name: IdentId(0),
            name_bytes: Box::from(&b""[..]),
            num_params,
            num_regs,
            code,
            consts,
            capture_regs: Vec::new(),
            closures: Vec::new(),
            span: Span::dummy(),
            ..Function::default()
        }
    }

    /// A single-function module whose lone function is `main`.
    fn module(main: Function) -> Module {
        Module {
            funcs: vec![main],
            classes: Vec::new(),
            main: 0,
        }
    }

    /// Run a module on a fresh test interpreter: the result of `{main}` and
    /// everything that reached the sink.
    fn run(m: &Module) -> (Result<Value, Unwind>, Vec<u8>) {
        let mut it = Interp::new_for_tests();
        it.load_module(m.clone());
        let r = it.run_main();
        it.finish_output();
        (r, it.test_output())
    }

    /// Run a module and decode its (binary-safe) stdout as UTF-8 for assertions.
    fn out_str(m: &Module) -> String {
        let (r, out) = run(m);
        r.unwrap();
        String::from_utf8(out).unwrap()
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
        let m = module(func(
            0,
            2,
            vec![
                Op::LoadBool { dst: 0, val: false },
                Op::JmpIfFalse { cond: 0, target: 4 },
                Op::LoadConst { dst: 1, k: 0 },
                Op::Echo { src: 1 },
                Op::LoadConst { dst: 1, k: 1 },
                Op::Echo { src: 1 },
                Op::Ret { src: None },
            ],
            vec![Const::Int(111), Const::Int(222)],
        ));
        assert_eq!(out_str(&m), "222");
    }

    #[test]
    fn call_returns_value() {
        // main: x = add2(20, 22); echo x   => "42"
        let main = func(
            0,
            3,
            vec![
                Op::LoadConst { dst: 0, k: 0 },
                Op::LoadConst { dst: 1, k: 1 },
                Op::Call {
                    dst: 2,
                    func: 1,
                    base: 0,
                    argc: 2,
                },
                Op::Echo { src: 2 },
                Op::Ret { src: None },
            ],
            vec![Const::Int(20), Const::Int(22)],
        );
        let add2 = func(
            2,
            3,
            vec![Op::Add { dst: 2, a: 0, b: 1 }, Op::Ret { src: Some(2) }],
            vec![],
        );
        let m = Module {
            funcs: vec![main, add2],
            classes: Vec::new(),
            main: 0,
        };
        assert_eq!(out_str(&m), "42");
    }

    #[test]
    fn recursive_call_factorial() {
        let main = func(
            0,
            2,
            vec![
                Op::LoadConst { dst: 0, k: 0 },
                Op::Call {
                    dst: 1,
                    func: 1,
                    base: 0,
                    argc: 1,
                },
                Op::Echo { src: 1 },
                Op::Ret { src: None },
            ],
            vec![Const::Int(5)],
        );
        let fact = func(
            1,
            6,
            vec![
                Op::LoadConst { dst: 1, k: 0 },
                Op::CmpLe { dst: 2, a: 0, b: 1 },
                Op::JmpIfFalse { cond: 2, target: 4 },
                Op::Ret { src: Some(1) },
                Op::Sub { dst: 3, a: 0, b: 1 },
                Op::Call {
                    dst: 4,
                    func: 1,
                    base: 3,
                    argc: 1,
                },
                Op::Mul { dst: 5, a: 0, b: 4 },
                Op::Ret { src: Some(5) },
            ],
            vec![Const::Int(1)],
        );
        let m = Module {
            funcs: vec![main, fact],
            classes: Vec::new(),
            main: 0,
        };
        assert_eq!(out_str(&m), "120");
    }

    #[test]
    fn division_by_zero_is_a_division_by_zero_error() {
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
        let (r, _) = run(&m);
        let err = r.unwrap_err();
        assert_eq!(err.kind(), Some(ErrorKind::DivisionByZeroError));
        assert_eq!(err.message(), Some("Division by zero"));
        // The site was captured at the frame boundary: `{main}` only.
        let Unwind::Pending(p) = err else { panic!() };
        assert_eq!(p.site.unwrap().trace, "#0 {main}");
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
        assert_eq!(run(&m).0.unwrap_err().message(), Some("Modulo by zero"));
    }

    #[test]
    fn uninitialized_register_reads_null() {
        let m = module(func(
            0,
            1,
            vec![Op::Echo { src: 0 }, Op::Ret { src: None }],
            vec![],
        ));
        assert_eq!(out_str(&m), "");
    }

    #[test]
    fn loop_with_backward_jump() {
        let m = module(func(
            0,
            5,
            vec![
                Op::LoadConst { dst: 0, k: 0 },
                Op::LoadConst { dst: 1, k: 1 },
                Op::LoadConst { dst: 2, k: 0 },
                Op::LoadConst { dst: 3, k: 2 },
                Op::CmpGt { dst: 4, a: 1, b: 2 },
                Op::JmpIfFalse { cond: 4, target: 9 },
                Op::Add { dst: 0, a: 0, b: 1 },
                Op::Sub { dst: 1, a: 1, b: 3 },
                Op::Jmp { target: 4 },
                Op::Echo { src: 0 },
                Op::Ret { src: None },
            ],
            vec![Const::Int(0), Const::Int(3), Const::Int(1)],
        ));
        assert_eq!(out_str(&m), "6");
    }

    #[test]
    fn concat_and_echo_string() {
        use rphp_value::Str;
        let m = module(func(
            0,
            3,
            vec![
                Op::LoadConst { dst: 0, k: 0 },
                Op::LoadConst { dst: 1, k: 1 },
                Op::Concat { dst: 0, a: 0, b: 1 },
                Op::LoadConst { dst: 1, k: 2 },
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
        let m = module(func(
            0,
            3,
            vec![
                Op::NewArray { dst: 0 },
                Op::LoadConst { dst: 1, k: 0 },
                Op::ArrayPush { arr: 0, value: 1 },
                Op::LoadConst { dst: 1, k: 1 },
                Op::ArrayPush { arr: 0, value: 1 },
                Op::LoadConst { dst: 1, k: 2 },
                Op::ArrayGet {
                    dst: 2,
                    base: 0,
                    key: 1,
                },
                Op::Echo { src: 2 },
                Op::Ret { src: None },
            ],
            vec![Const::Int(10), Const::Int(20), Const::Int(1)],
        ));
        assert_eq!(out_str(&m), "20");
    }

    #[test]
    fn foreach_sums_values() {
        let m = module(func(
            0,
            6,
            vec![
                Op::NewArray { dst: 0 },
                Op::LoadConst { dst: 5, k: 0 },
                Op::ArrayPush { arr: 0, value: 5 },
                Op::LoadConst { dst: 5, k: 1 },
                Op::ArrayPush { arr: 0, value: 5 },
                Op::LoadConst { dst: 5, k: 2 },
                Op::ArrayPush { arr: 0, value: 5 },
                Op::LoadConst { dst: 1, k: 3 },
                Op::Move { dst: 3, src: 0 },
                Op::LoadConst { dst: 4, k: 3 },
                Op::ForeachNext {
                    arr: 3,
                    cursor: 4,
                    key_dst: 5,
                    val_dst: 2,
                    target: 13,
                },
                Op::Add { dst: 1, a: 1, b: 2 },
                Op::Jmp { target: 10 },
                Op::Echo { src: 1 },
                Op::Ret { src: None },
            ],
            vec![Const::Int(3), Const::Int(4), Const::Int(5), Const::Int(0)],
        ));
        assert_eq!(out_str(&m), "12");
    }

    #[test]
    fn echo_preserves_raw_bytes() {
        use rphp_value::Str;
        let m = module(func(
            0,
            1,
            vec![
                Op::LoadConst { dst: 0, k: 0 },
                Op::Echo { src: 0 },
                Op::Ret { src: None },
            ],
            vec![Const::Str(Str::new(&[0xFF, 0x00, 0x41]))],
        ));
        assert_eq!(run(&m).1, vec![0xFF, 0x00, 0x41]);
    }

    // ---- diagnostics channel ------------------------------------------------

    #[test]
    fn undefined_array_key_warns_like_php() {
        use rphp_value::Str;
        // $a = []; echo $a["k"]; echo $a[5];  with a line table.
        let mut f = func(
            0,
            3,
            vec![
                Op::NewArray { dst: 0 },
                Op::LoadConst { dst: 1, k: 0 },
                Op::ArrayGet {
                    dst: 2,
                    base: 0,
                    key: 1,
                },
                Op::LoadConst { dst: 1, k: 1 },
                Op::ArrayGet {
                    dst: 2,
                    base: 0,
                    key: 1,
                },
                Op::LoadNull { dst: 0 },
                Op::ArrayGet {
                    dst: 2,
                    base: 0,
                    key: 1,
                },
                Op::Ret { src: None },
            ],
            vec![Const::Str(Str::new(b"k")), Const::Int(5)],
        );
        f.lines = vec![1, 2, 2, 3, 3, 4, 4, 4];
        let m = module(f);
        let mut it = Interp::new_for_tests();
        it.script_name = "/tmp/t.php".into();
        it.load_module(m);
        it.run_main().unwrap();
        assert_eq!(
            String::from_utf8(it.test_output()).unwrap(),
            "\nWarning: Undefined array key \"k\" in /tmp/t.php on line 2\n\
             \nWarning: Undefined array key 5 in /tmp/t.php on line 3\n\
             \nWarning: Trying to access array offset on null in /tmp/t.php on line 4\n"
        );
        let last = it.last_error.clone().unwrap();
        assert_eq!(last.kind, E_WARNING);
        assert_eq!(last.line, 4);
    }

    fn h_false(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
        let line = format!(
            "H[{}] {} @{}:{} er={}\n",
            args[0].to_int(),
            args[1].to_php_string(),
            args[2].to_php_string(),
            args[3].to_int(),
            ctx.effective_error_reporting()
        );
        ctx.echo(line.as_bytes());
        Ok(Value::Bool(false))
    }

    fn h_true(ctx: &mut Ctx, args: &mut [Value]) -> NativeResult {
        let line = format!("handled: {}\n", args[1].to_php_string());
        ctx.echo(line.as_bytes());
        Ok(Value::Bool(true))
    }

    fn h_throw(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
        Err(Unwind::exception("ErrorException", args[1].to_php_string()))
    }

    fn interp_with_handlers() -> Interp {
        let mut it = Interp::new_for_tests();
        Registry(&mut it).functions(&[
            nf!("h_false", 4, Some(4), h_false),
            nf!("h_true", 4, Some(4), h_true),
            nf!("h_throw", 4, Some(4), h_throw),
        ]);
        it
    }

    #[test]
    fn emit_error_display_silence_and_mask() {
        let mut it = Interp::new_for_tests();
        it.warn("w1").unwrap();
        it.silence += 1;
        it.warn("silenced").unwrap();
        it.silence -= 1;
        assert_eq!(
            it.last_error.as_ref().unwrap().message,
            "silenced",
            "recorded even when silenced"
        );
        it.error_reporting = E_ALL & !E_NOTICE;
        it.notice("masked").unwrap();
        it.deprecated("d").unwrap();
        assert_eq!(
            String::from_utf8(it.take_test_output()).unwrap(),
            "\nWarning: w1 in Command line code on line 0\n\nDeprecated: d in Command line code on line 0\n"
        );
        it.ini_set("display_errors", "0");
        it.warn("hidden").unwrap();
        assert_eq!(it.take_test_output(), b"");
    }

    #[test]
    fn emit_error_user_handler_false_falls_through_true_swallows_throw_propagates() {
        let mut it = interp_with_handlers();
        it.error_handler.push((Value::string(b"h_false"), E_ALL));
        it.silence += 1;
        it.warn("u").unwrap();
        it.silence -= 1;
        // Under `@` the handler still runs and sees the masked mask; the
        // fall-through display is then suppressed by that same mask.
        assert_eq!(
            String::from_utf8(it.take_test_output()).unwrap(),
            format!("H[2] u @Command line code:0 er={SILENCE_MASK}\n")
        );
        it.warn("v").unwrap();
        assert_eq!(
            String::from_utf8(it.take_test_output()).unwrap(),
            "H[2] v @Command line code:0 er=30719\n\nWarning: v in Command line code on line 0\n"
        );
        it.error_handler.push((Value::string(b"h_true"), E_ALL));
        it.last_error = None;
        it.warn("w").unwrap();
        assert_eq!(
            String::from_utf8(it.take_test_output()).unwrap(),
            "handled: w\n"
        );
        assert!(it.last_error.is_none(), "a handled error is not recorded");
        // A handler mask that excludes the level skips the handler.
        it.error_handler.push((Value::string(b"h_true"), E_NOTICE));
        it.warn("x").unwrap();
        assert_eq!(
            String::from_utf8(it.take_test_output()).unwrap(),
            "\nWarning: x in Command line code on line 0\n"
        );
        it.error_handler.push((Value::string(b"h_throw"), E_ALL));
        let err = it.warn("boom").unwrap_err();
        assert_eq!(err.kind(), Some(ErrorKind::Exception("ErrorException")));
        assert_eq!(err.message(), Some("boom"));
        // set_error_handler(null) disables; restore pops back.
        it.error_handler.push((Value::Null, E_ALL));
        it.warn("plain").unwrap();
        assert_eq!(
            String::from_utf8(it.take_test_output()).unwrap(),
            "\nWarning: plain in Command line code on line 0\n"
        );
    }

    #[test]
    fn fatal_level_exits_255_after_display() {
        let mut it = Interp::new_for_tests();
        let err = it.emit_error(ErrLevel::UserError, "fatal!").unwrap_err();
        assert!(matches!(err, Unwind::Exit(255)));
        assert_eq!(
            String::from_utf8(it.test_output()).unwrap(),
            "\nFatal error: fatal! in Command line code on line 0\nStack trace:\n#0 {main}\n"
        );
    }

    #[test]
    fn render_uncaught_matches_php_cli_shape() {
        let mut it = Interp::new_for_tests();
        it.script_name = "/abs/file.php".into();
        let p = PendingThrow {
            kind: ErrorKind::Error,
            message: "Call to undefined function foo()".into(),
            site: Some(Box::new(FaultSite {
                file: "/abs/file.php".into(),
                line: 3,
                trace: "#0 {main}".into(),
            })),
        };
        it.render_uncaught(&p);
        assert_eq!(
            String::from_utf8(it.test_output()).unwrap(),
            "\nFatal error: Uncaught Error: Call to undefined function foo() in /abs/file.php:3\n\
             Stack trace:\n#0 {main}\n  thrown in /abs/file.php on line 3\n"
        );
        let last = it.last_error.unwrap();
        assert_eq!(last.kind, E_ERROR);
        assert_eq!(last.line, 3);
        assert!(last.message.ends_with("  thrown"));
    }

    #[test]
    fn output_before_a_fault_reaches_the_sink_and_shutdown_runs() {
        // echo "before"; 1/0;
        let m = module(func(
            0,
            3,
            vec![
                Op::LoadConst { dst: 0, k: 0 },
                Op::Echo { src: 0 },
                Op::LoadConst { dst: 0, k: 1 },
                Op::LoadConst { dst: 1, k: 2 },
                Op::Div { dst: 2, a: 0, b: 1 },
                Op::Ret { src: None },
            ],
            vec![
                Const::Str(rphp_value::Str::new(b"before\n")),
                Const::Int(1),
                Const::Int(0),
            ],
        ));
        let mut it = interp_with_handlers();
        it.load_module(m);
        it.ob_start(None, 0, PHP_OUTPUT_HANDLER_STDFLAGS); // even buffered output survives
        let code = match it.run_main() {
            Ok(_) => 0,
            Err(u) => it.handle_top_level_unwind(u),
        };
        assert_eq!(code, 255);
        it.finish_output();
        assert_eq!(
            String::from_utf8(it.test_output()).unwrap(),
            "before\n\nFatal error: Uncaught DivisionByZeroError: Division by zero in Command line code:0\n\
             Stack trace:\n#0 {main}\n  thrown in Command line code on line 0\n"
        );
    }

    #[test]
    fn shutdown_functions_run_in_order_and_a_fault_stops_the_rest() {
        let mut it = interp_with_handlers();
        it.load_module(module(func(0, 0, vec![Op::Ret { src: None }], vec![])));
        it.run_main().unwrap();
        it.shutdown.push((
            Value::string(b"h_true"),
            vec![
                Value::Int(0),
                Value::string(b"s1"),
                Value::Null,
                Value::Int(0),
            ],
        ));
        it.shutdown.push((
            Value::string(b"h_throw"),
            vec![
                Value::Int(0),
                Value::string(b"s2"),
                Value::Null,
                Value::Int(0),
            ],
        ));
        it.shutdown.push((
            Value::string(b"h_true"),
            vec![
                Value::Int(0),
                Value::string(b"s3"),
                Value::Null,
                Value::Int(0),
            ],
        ));
        let code = it.run_shutdown_functions(0);
        assert_eq!(code, 255);
        let out = String::from_utf8(it.test_output()).unwrap();
        assert!(out.starts_with("handled: s1\n\nFatal error: Uncaught ErrorException: s2 in Command line code:0\nStack trace:\n#0 [internal function]: h_throw(0, 's2', NULL, 0)\n#1 {main}\n  thrown"), "{out}");
        assert!(!out.contains("s3"));
        assert!(it.frames().is_empty());
    }

    // ---- output stack through the interpreter --------------------------------

    fn upper(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
        let phase = args[1].to_int();
        Ok(Value::string(
            format!("{}|{phase}", args[0].to_php_string().to_uppercase()).as_bytes(),
        ))
    }

    #[test]
    fn ob_levels_with_handlers() {
        let mut it = Interp::new_for_tests();
        Registry(&mut it).function(nf!("upper", 2, Some(2), upper));
        it.echo(b"x");
        it.ob_start(
            Some(Value::string(b"upper")),
            0,
            PHP_OUTPUT_HANDLER_STDFLAGS,
        );
        it.echo(b"abc");
        assert_eq!(it.out.level(), 1);
        assert_eq!(it.ob_flush_top(true).unwrap().unwrap(), b"abc");
        assert_eq!(it.out.level(), 0);
        // START|FINAL = 9, as php reports on ob_end_flush().
        assert_eq!(it.take_test_output(), b"xABC|9");
        it.ob_start(None, 0, PHP_OUTPUT_HANDLER_STDFLAGS);
        it.echo(b"in");
        it.out().extend_from_slice(b"+native");
        assert_eq!(it.out.top_contents().unwrap(), b"in+native");
        assert_eq!(it.ob_discard_top().unwrap().unwrap(), b"in+native");
        assert!(it.ob_discard_top().unwrap().is_none());
        assert_eq!(it.take_test_output(), b"");
        // ob_flush keeps the level; the handler is not restarted.
        it.ob_start(
            Some(Value::string(b"upper")),
            0,
            PHP_OUTPUT_HANDLER_STDFLAGS,
        );
        it.echo(b"a");
        it.ob_flush_top(false).unwrap();
        it.echo(b"b");
        it.ob_flush_top(true).unwrap();
        assert_eq!(it.take_test_output(), b"A|5B|8");
    }

    // ---- registry, constants, ini ----------------------------------------------

    fn strlen(_: &mut Ctx, args: &mut [Value]) -> NativeResult {
        Ok(Value::Int(args[0].to_php_bytes().len() as i64))
    }

    #[test]
    fn natives_resolve_case_insensitively_and_check_arity() {
        let mut it = Interp::new_for_tests();
        let id = Registry(&mut it).function(nf!("strlen", 1, Some(1), strlen));
        assert_eq!(it.native_by_name(b"STRLEN"), Some(id));
        assert_eq!(
            it.call_function(b"StrLen", &[Value::string(b"abcd")])
                .unwrap(),
            Value::Int(4)
        );
        let err = it.call_function(b"strlen", &[]).unwrap_err();
        assert_eq!(err.kind(), Some(ErrorKind::ArgumentCountError));
        assert_eq!(
            err.message(),
            Some("strlen() expects exactly 1 argument, 0 given")
        );
        let err = it.call_function(b"nope", &[]).unwrap_err();
        assert_eq!(err.message(), Some("Call to undefined function nope()"));
        // Re-registering keeps the id.
        assert_eq!(
            Registry(&mut it).function(nf!("STRLEN", 1, Some(1), strlen)),
            id
        );
        assert_eq!(it.natives().len(), 1);
    }

    #[test]
    fn constants_and_ini() {
        let mut it = Interp::new_for_tests();
        Registry(&mut it).constant("PHP_EOL", Value::string(b"\n"));
        assert_eq!(it.constant(b"PHP_EOL"), Some(Value::string(b"\n")));
        assert!(it.defined(b"PHP_EOL"));
        assert!(!it.defined(b"php_eol"), "constants are case-sensitive");
        assert!(it.define(b"X", Value::Int(1)));
        assert!(!it.define(b"X", Value::Int(2)));
        assert_eq!(it.constant(b"X"), Some(Value::Int(1)));
        assert_eq!(it.ini_get("precision"), Some("14"));
        assert_eq!(it.ini_set("precision", "10"), Some("14".into()));
        assert_eq!(it.ini_set("nope.x", "1"), None);
        assert_eq!(
            it.ini_set("error_reporting", "E_ALL & ~E_WARNING"),
            Some(String::new())
        );
        assert_eq!(it.error_reporting, E_ALL & !E_WARNING);
        assert_eq!(it.set_error_reporting(E_ALL), E_ALL & !E_WARNING);
        assert_eq!(it.ini_get("error_reporting"), Some("30719"));
    }
}
