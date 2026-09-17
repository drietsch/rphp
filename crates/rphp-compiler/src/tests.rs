//! Unit tests: every snippet goes through the front end (`parse_v2`) so the
//! tests exercise the tree shapes the adapter really produces.

use super::*;
use rphp_bytecode::{CodeAddr, Const, Op};
use rphp_parser::{parse_v2, ParseOptions};
use rphp_span::FileId;
use rphp_value::Str;

/// The natives the tests bind to: `strlen`, `count`, `sort` (by-ref).
struct TestNatives;

impl KnownFunctions for TestNatives {
    fn native(&self, name: &[u8]) -> Option<NativeSig> {
        Some(match name.to_ascii_lowercase().as_slice() {
            b"strlen" => NativeSig {
                id: 0,
                min_args: 1,
                max_args: Some(1),
                by_ref: 0,
            },
            b"count" => NativeSig {
                id: 1,
                min_args: 1,
                max_args: Some(2),
                by_ref: 0,
            },
            b"sort" => NativeSig {
                id: 2,
                min_args: 1,
                max_args: Some(2),
                by_ref: 0b1,
            },
            _ => return None,
        })
    }
}

fn parse(src: &str, interner: &mut Interner) -> Program {
    let parsed = parse_v2(src.as_bytes(), ParseOptions::new(FileId(0)), interner);
    assert!(
        parsed.diagnostics.iter().all(|d| !d.is_error()),
        "snippet must parse cleanly: {:?}",
        parsed.diagnostics
    );
    parsed.program
}

fn compile_src(src: &str) -> Result<Module, Vec<Diagnostic>> {
    let mut interner = Interner::new();
    let program = parse(src, &mut interner);
    compile(&program, &interner, &CompileOptions::new(&TestNatives))
}

fn compile_ok(src: &str) -> Module {
    compile_src(src).unwrap_or_else(|d| panic!("expected successful compile, got {d:#?}"))
}

fn compile_err(src: &str) -> Vec<Diagnostic> {
    compile_src(src).expect_err("expected a diagnostic")
}

fn has_code(diags: &[Diagnostic], code: &str) -> bool {
    diags.iter().any(|d| d.code == code && d.is_error())
}

/// The E0300 messages of a failed compile, for asserting what was reported.
fn unsupported_messages(src: &str) -> Vec<String> {
    let diags = compile_err(src);
    assert!(
        diags.iter().all(|d| d.code == UNSUPPORTED_CONSTRUCT),
        "only E0300 expected, got {diags:#?}"
    );
    diags.into_iter().map(|d| d.message).collect()
}

#[test]
fn line_tables_follow_statements_and_calls() {
    // Two statements on lines 1 and 3.
    let src = "<?php echo 1;\n\necho 2;";
    let mut interner = Interner::new();
    let program = parse(src, &mut interner);
    let line_of = |off: u32| {
        1 + src.as_bytes()[..off as usize]
            .iter()
            .filter(|&&b| b == b'\n')
            .count() as u32
    };
    let opts = CompileOptions {
        natives: &TestNatives,
        line_of: Some(&line_of),
    };
    let m = compile(&program, &interner, &opts).unwrap();
    let main = m.func(0);
    assert_eq!(main.lines.len(), main.code.len());
    assert_eq!(main.lines, vec![1, 1, 3, 3, 3]);
    // Without `line_of` the table stays empty (contract: "or empty").
    let m = compile_ok("<?php");
    assert!(m.func(0).lines.is_empty());
}

#[test]
fn echo_add_emits_add_and_echo() {
    let m = compile_ok("<?php echo 1 + 2;");

    assert_eq!(m.main, 0);
    assert_eq!(m.funcs.len(), 1);
    let main = m.func(0);
    assert_eq!(main.num_params, 0);
    assert_eq!(main.consts, vec![Const::Int(1), Const::Int(2)]);
    assert_eq!(
        main.code,
        vec![
            Op::LoadConst { dst: 0, k: 0 },
            Op::LoadConst { dst: 1, k: 1 },
            Op::Add { dst: 0, a: 0, b: 1 },
            Op::Echo { src: 0 },
            Op::Ret { src: None },
        ]
    );
    assert_eq!(main.num_regs, 2);
}

#[test]
fn every_function_ends_with_ret() {
    let m = compile_ok("<?php echo 7;");
    assert!(matches!(m.func(0).code.last(), Some(Op::Ret { src: None })));
}

#[test]
fn assignment_then_use() {
    let m = compile_ok("<?php $x = 5; echo $x;");
    let main = m.func(0);
    // $x is register 0 (the only variable, no params).
    assert!(main.code.iter().any(|op| matches!(op, Op::Echo { src: 0 })));
    // The constant 5 is loaded and the value reaches register 0.
    assert_eq!(main.consts, vec![Const::Int(5)]);
    assert!(main
        .code
        .iter()
        .any(|op| matches!(op, Op::Move { dst: 0, .. } | Op::LoadConst { dst: 0, .. })));
}

#[test]
fn self_referential_assignment_reads_before_write() {
    // $x = $x + 1;  — old $x must be read before the result lands in reg 0.
    let m = compile_ok("<?php $x = $x + 1;");
    let main = m.func(0);
    let add = main
        .code
        .iter()
        .find_map(|op| match op {
            Op::Add { dst, a, b } => Some((*dst, *a, *b)),
            _ => None,
        })
        .expect("expected an Add");
    assert!(add.1 == 0 || add.2 == 0, "Add should read $x (reg 0)");
    assert_ne!(add.0, 0, "Add result should go to a temp, not clobber $x");
}

#[test]
fn call_resolves_to_func_id_and_arity() {
    let m = compile_ok("<?php function foo($a) { return $a; } foo(7);");

    assert_eq!(m.funcs.len(), 2);
    // foo is FuncId 1 with one param.
    let foo_fn = m.func(1);
    assert_eq!(foo_fn.num_params, 1);
    assert!(matches!(
        foo_fn.code.first(),
        Some(Op::Ret { src: Some(0) })
    ));

    // main contains a Call to FuncId 1 with argc 1.
    let main = m.func(0);
    let callop = main
        .code
        .iter()
        .find_map(|op| match op {
            Op::Call {
                func, base, argc, ..
            } => Some((*func, *base, *argc)),
            _ => None,
        })
        .expect("expected a Call op");
    assert_eq!(callop.0, 1, "resolved FuncId");
    assert_eq!(callop.2, 1, "argc");
}

#[test]
fn forward_reference_resolves() {
    // The call appears before the declaration.
    let m = compile_ok("<?php foo(); function foo() {}");
    let main = m.func(0);
    assert!(main.code.iter().any(|op| matches!(
        op,
        Op::Call {
            func: 1,
            argc: 0,
            ..
        }
    )));
}

#[test]
fn declarations_in_top_level_blocks_and_global_namespace_are_hoisted() {
    let m = compile_ok("<?php { function foo() {} } foo();");
    assert_eq!(m.funcs.len(), 2);
    let m = compile_ok("<?php namespace { { class C {} } function foo() {} foo(); new C; }");
    assert_eq!(m.funcs.len(), 2);
    assert_eq!(m.classes.len(), 1);
}

#[test]
fn undefined_function_diagnoses() {
    let diags = compile_err("<?php bar(1);");
    assert!(has_code(&diags, codes::UNDEFINED_FUNCTION));
}

#[test]
fn native_call_lowers_to_call_native() {
    // strlen("x"); — a builtin, with no matching user function.
    let m = compile_ok("<?php strlen('x');");
    assert!(m
        .func(0)
        .code
        .iter()
        .any(|op| matches!(op, Op::CallNative { argc: 1, .. })));
    // The builtin is not lowered into a user `Function`.
    assert_eq!(m.funcs.len(), 1);
}

#[test]
fn fully_qualified_native_call_resolves() {
    let m = compile_ok("<?php \\strlen('x');");
    assert!(m
        .func(0)
        .code
        .iter()
        .any(|op| matches!(op, Op::CallNative { argc: 1, .. })));
}

#[test]
fn user_function_shadows_builtin_of_same_name() {
    // The user def wins, so the call is a user `Call`, not a `CallNative`.
    let m = compile_ok("<?php function count() { return 1; } count();");
    let main = &m.func(0).code;
    assert!(main.iter().any(|op| matches!(op, Op::Call { func: 1, .. })));
    assert!(!main.iter().any(|op| matches!(op, Op::CallNative { .. })));
}

#[test]
fn wrong_native_arity_diagnoses() {
    // strlen takes exactly one argument.
    let diags = compile_err("<?php strlen('x', 'x');");
    assert!(has_code(&diags, codes::WRONG_ARG_COUNT));
}

#[test]
fn by_ref_builtin_writes_back_to_variable() {
    // sort takes its array by reference.
    let m = compile_ok("<?php $a = []; sort($a);");
    let code = &m.func(0).code;
    let call = code
        .iter()
        .position(|op| matches!(op, Op::CallNative { .. }))
        .unwrap();
    // $a is register 0; after the call its register is written back.
    assert!(code[call + 1..]
        .iter()
        .any(|op| matches!(op, Op::Move { dst: 0, .. })));
}

#[test]
fn by_ref_non_variable_diagnoses() {
    // A by-ref parameter requires a variable, not a literal.
    let diags = compile_err("<?php sort([1]);");
    assert!(has_code(&diags, BY_REF_NOT_VARIABLE));
}

#[test]
fn wrong_arg_count_diagnoses() {
    // Two args for a one-param function.
    let diags = compile_err("<?php function foo($a) {} foo(1, 2);");
    assert!(has_code(&diags, codes::WRONG_ARG_COUNT));
}

#[test]
fn duplicate_function_diagnoses() {
    let diags = compile_err("<?php function foo() {} function foo() {}");
    assert!(has_code(&diags, REDECLARED_FUNCTION));
}

#[test]
fn logical_and_short_circuits_with_branches() {
    let m = compile_ok("<?php echo true && false;");
    let code = &m.func(0).code;
    // A short-circuit branch and a join jump are present, plus the bool
    // normalization (two Nots), and no boolean opcode was invented.
    assert!(code.iter().any(|op| matches!(op, Op::JmpIfFalse { .. })));
    assert!(code.iter().any(|op| matches!(op, Op::Jmp { .. })));
    assert_eq!(
        code.iter()
            .filter(|op| matches!(op, Op::Not { .. }))
            .count(),
        2
    );
    assert_branch_targets_in_range(code);
}

#[test]
fn logical_or_uses_jmp_if_true() {
    let m = compile_ok("<?php echo false || true;");
    let code = &m.func(0).code;
    assert!(code.iter().any(|op| matches!(op, Op::JmpIfTrue { .. })));
    assert_branch_targets_in_range(code);
}

#[test]
fn if_else_backpatches() {
    let m = compile_ok("<?php if (1) { echo 1; } else { echo 2; }");
    let code = &m.func(0).code;
    assert!(code.iter().any(|op| matches!(op, Op::JmpIfFalse { .. })));
    assert!(code.iter().any(|op| matches!(op, Op::Jmp { .. })));
    assert_branch_targets_in_range(code);
}

#[test]
fn elseif_nests_as_else_if() {
    let m = compile_ok("<?php if (1) { echo 1; } elseif (2) { echo 2; } else { echo 3; }");
    let code = &m.func(0).code;
    // Two conditions, two false-branches, two join jumps.
    assert_eq!(
        code.iter()
            .filter(|op| matches!(op, Op::JmpIfFalse { .. }))
            .count(),
        2
    );
    assert_eq!(
        code.iter()
            .filter(|op| matches!(op, Op::Jmp { .. }))
            .count(),
        2
    );
    assert_branch_targets_in_range(code);
}

#[test]
fn while_jumps_backward() {
    let m = compile_ok("<?php while (1) { echo 1; }");
    let code = &m.func(0).code;
    // The back-edge jump targets an index at or before its own position.
    let back_jmp = code.iter().enumerate().find_map(|(i, op)| match op {
        Op::Jmp { target } if (*target as usize) <= i => Some(*target),
        _ => None,
    });
    assert!(back_jmp.is_some(), "expected a backward loop jump");
    assert!(code.iter().any(|op| matches!(op, Op::JmpIfFalse { .. })));
    assert_branch_targets_in_range(code);
}

#[test]
fn unary_and_comparison_ops() {
    let m = compile_ok("<?php echo -1; echo !0; echo 1 <=> 2; echo 1 < 2;");
    let code = &m.func(0).code;
    assert!(code.iter().any(|op| matches!(op, Op::Neg { .. })));
    assert!(code.iter().any(|op| matches!(op, Op::Not { .. })));
    assert!(code.iter().any(|op| matches!(op, Op::Spaceship { .. })));
    assert!(code.iter().any(|op| matches!(op, Op::CmpLt { .. })));
}

#[test]
fn string_literal_and_concat() {
    let m = compile_ok("<?php echo \"hi\" . '!';");
    let main = m.func(0);
    // Both string literals reach the constant pool as Const::Str.
    assert!(main.consts.contains(&Const::Str(Str::new(b"hi"))));
    assert!(main.consts.contains(&Const::Str(Str::new(b"!"))));
    // A Concat op was emitted (not an Add).
    assert!(main.code.iter().any(|op| matches!(op, Op::Concat { .. })));
    assert!(main.code.iter().any(|op| matches!(op, Op::Echo { .. })));
}

#[test]
fn interpolation_is_a_concat_chain_seeded_with_empty_string() {
    // "a $x" => Concat(Concat("", "a "), $x): two Concats, an empty-string seed.
    let m = compile_ok("<?php echo \"a $x\";");
    let main = m.func(0);
    assert_eq!(
        main.code
            .iter()
            .filter(|op| matches!(op, Op::Concat { .. }))
            .count(),
        2
    );
    assert_eq!(main.consts[0], Const::Str(Str::new(b"")));
    assert!(main.consts.contains(&Const::Str(Str::new(b"a "))));
    // A lone "$x" is still stringified through the seed.
    let m = compile_ok("<?php echo \"$x\";");
    assert!(m
        .func(0)
        .code
        .iter()
        .any(|op| matches!(op, Op::Concat { .. })));
}

#[test]
fn array_literal_emits_new_and_fills() {
    let m = compile_ok("<?php [1, 'k' => 2];");
    let code = &m.func(0).code;
    assert!(code.iter().any(|op| matches!(op, Op::NewArray { .. })));
    assert!(code.iter().any(|op| matches!(op, Op::ArrayPush { .. })));
    assert!(code.iter().any(|op| matches!(op, Op::ArraySet { .. })));
}

#[test]
fn append_lowers_to_array_push() {
    let m = compile_ok("<?php $a[] = 5;");
    assert!(m
        .func(0)
        .code
        .iter()
        .any(|op| matches!(op, Op::ArrayPush { .. })));
}

#[test]
fn foreach_lowers_to_foreach_next() {
    let m = compile_ok("<?php foreach ($a as $k => $v) { echo $v; }");
    let code = &m.func(0).code;
    assert!(code.iter().any(|op| matches!(op, Op::ForeachNext { .. })));
    // The exhaustion target must be a valid instruction index.
    let n = code.len() as CodeAddr;
    for op in code {
        if let Op::ForeachNext { target, .. } = op {
            assert!(*target <= n);
        }
    }
}

#[test]
fn nested_array_write_diagnoses() {
    // Nested lvalue write is not supported yet.
    let diags = compile_err("<?php $a[0][1] = 5;");
    assert!(has_code(&diags, NESTED_ARRAY_WRITE));
}

#[test]
fn params_occupy_low_registers() {
    let m = compile_ok("<?php function add($a, $b) { return $a + $b; }");
    let add_fn = m.func(1);
    assert_eq!(add_fn.num_params, 2);
    // $a -> reg 0, $b -> reg 1; their Add reads exactly those.
    let add_op = add_fn
        .code
        .iter()
        .find_map(|op| match op {
            Op::Add { a, b, .. } => Some((*a, *b)),
            _ => None,
        })
        .expect("expected Add");
    assert_eq!(add_op, (0, 1));
    assert!(add_fn.num_regs >= 2);
}

#[test]
fn typed_params_and_returns_are_ignored_metadata() {
    let m = compile_ok(
        "<?php function add(int $a, ?string $b): int|false { return $a; }\n\
         class C { public int $v = 1; private ?C $n = null; public function m(C $c): void {} }",
    );
    assert_eq!(m.func(1).num_params, 2);
    assert_eq!(m.classes[0].props.len(), 2);
}

#[test]
fn closure_and_arrow_fn_capture_by_value() {
    let m = compile_ok(
        "<?php $a = 1; $b = 2; $f = function ($x) use ($a) { return $x + $a; }; $g = fn($y) => $y * $b;",
    );
    // main, then the two closures appended.
    assert_eq!(m.funcs.len(), 3);
    let main = m.func(0);
    assert_eq!(main.closures.len(), 2);
    assert_eq!(main.closures[0].src_regs.len(), 1);
    assert_eq!(main.closures[1].src_regs.len(), 1);
    let arrow = m.func(2);
    assert_eq!(arrow.num_params, 1);
    assert_eq!(arrow.capture_regs.len(), 1);
    assert!(arrow
        .code
        .iter()
        .any(|op| matches!(op, Op::Ret { src: Some(_) })));
}

#[test]
fn nested_arrow_fn_captures_propagate_outwards() {
    // The inner arrow function reads $z; the outer one must capture it too.
    let m = compile_ok("<?php $z = 1; $f = fn($a) => fn($b) => $a + $b + $z;");
    let main = m.func(0);
    assert_eq!(main.closures[0].src_regs, vec![0]);
}

#[test]
fn class_compiles_methods_and_binds_this_to_reg_zero() {
    let m = compile_ok(
        "<?php class C { public $v = 1; function get() { return $this->v; } }\n\
         $c = new C(); $c->get();",
    );

    // One class, with its property default folded to a constant value.
    assert_eq!(m.classes.len(), 1);
    assert_eq!(m.classes[0].props.len(), 1);
    assert_eq!(m.classes[0].props[0].default, rphp_value::Value::Int(1));
    assert_eq!(m.classes[0].methods.len(), 1);

    // main instantiates and dispatches.
    let main = &m.func(0).code;
    assert!(main.iter().any(|op| matches!(op, Op::New { class: 0, .. })));
    assert!(main.iter().any(|op| matches!(op, Op::MethodCall { .. })));

    // The method body reads `$this` (register 0) for the property access, and
    // the method frame reserves register 0 for `$this` (num_params == 1).
    let fid = m.classes[0].methods[0].func;
    let method = m.func(fid);
    assert_eq!(
        method.num_params, 1,
        "$this occupies the sole parameter slot"
    );
    assert!(method
        .code
        .iter()
        .any(|op| matches!(op, Op::PropGet { obj: 0, .. })));
}

#[test]
fn negative_and_multi_item_property_defaults() {
    let m = compile_ok("<?php class C { public $a = -1, $b = 'x'; protected $c; var $d = 1.5; }");
    let props = &m.classes[0].props;
    assert_eq!(props.len(), 4);
    assert_eq!(props[0].default, rphp_value::Value::Int(-1));
    assert_eq!(props[2].default, rphp_value::Value::Null);
    assert_eq!(props[2].visibility, rphp_bytecode::Visibility::Protected);
    assert_eq!(props[3].visibility, rphp_bytecode::Visibility::Public);
}

#[test]
fn constant_shaped_but_unfolded_default_is_e0300() {
    // (A non-constant default such as `$x` is already a parse error in the
    // front end; `NON_CONST_PROP_DEFAULT` stays as defence in depth.)
    for src in [
        "<?php class C { public $a = []; }",
        "<?php class C { public $a = PHP_EOL; }",
        "<?php class C { public $a = 1 + 2; }",
    ] {
        let diags = compile_err(src);
        assert!(has_code(&diags, UNSUPPORTED_CONSTRUCT), "{src}: {diags:#?}");
        assert!(
            !has_code(&diags, NON_CONST_PROP_DEFAULT),
            "{src}: {diags:#?}"
        );
    }
}

#[test]
fn undefined_class_diagnoses() {
    let diags = compile_err("<?php new Nope();");
    assert!(has_code(&diags, UNDEFINED_CLASS));
}

#[test]
fn scoped_calls_and_instanceof() {
    let m = compile_ok(
        "<?php class A { function m() { return 1; } }\n\
         class B extends A { function m() { return parent::m() + self::m(); } function t($o) { return $o instanceof A && $o instanceof Nope; } }\n\
         A::m();",
    );
    let b_m = m.func(m.classes[1].methods[0].func);
    assert_eq!(
        b_m.code
            .iter()
            .filter(|op| matches!(op, Op::StaticCall { .. }))
            .count(),
        2
    );
    let b_t = m.func(m.classes[1].methods[1].func);
    assert!(b_t
        .code
        .iter()
        .any(|op| matches!(op, Op::InstanceOf { class: 0, .. })));
    // An unknown class name in `instanceof` is a constant false, not an error.
    assert!(b_t
        .code
        .iter()
        .any(|op| matches!(op, Op::LoadBool { val: false, .. })));
    assert!(m
        .func(0)
        .code
        .iter()
        .any(|op| matches!(op, Op::StaticCall { .. })));
    let diags = compile_err("<?php self::m();");
    assert!(has_code(&diags, INVALID_SCOPE));
}

#[test]
fn unsupported_constructs_report_e0300_with_a_description() {
    let msgs = unsupported_messages("<?php $a ??= 1; for (;;) {} match (1) { default => 2 };");
    assert!(
        msgs.iter()
            .any(|m| m.contains("compound assignment `coalesce=`")),
        "{msgs:?}"
    );
    assert!(msgs.iter().any(|m| m.contains("for loop")), "{msgs:?}");
    assert!(msgs.iter().any(|m| m.contains("match")), "{msgs:?}");
    assert!(msgs
        .iter()
        .all(|m| m.starts_with("unsupported construct: ") && m.ends_with(" (not lowered yet)")));

    let msgs =
        unsupported_messages("<?php namespace Foo; use Bar\\Baz; const X = 1; echo PHP_EOL;");
    assert!(
        msgs.iter().any(|m| m.contains("namespace declaration")),
        "{msgs:?}"
    );

    let msgs = unsupported_messages(
        "<?php function f($a = 1, &$b, ...$c) {} static fn() => 1; $o?->p; static::m();",
    );
    assert!(
        msgs.iter().any(|m| m.contains("default parameter value")),
        "{msgs:?}"
    );
    assert!(
        msgs.iter().any(|m| m.contains("by-reference parameter")),
        "{msgs:?}"
    );
    assert!(
        msgs.iter().any(|m| m.contains("variadic parameter")),
        "{msgs:?}"
    );
    assert!(
        msgs.iter().any(|m| m.contains("static arrow function")),
        "{msgs:?}"
    );
    assert!(
        msgs.iter().any(|m| m.contains("nullsafe property fetch")),
        "{msgs:?}"
    );
    assert!(
        msgs.iter().any(|m| m.contains("late static binding")),
        "{msgs:?}"
    );

    let msgs = unsupported_messages(
        "<?php interface I {} trait T {} enum E {} abstract class A { const K = 1; static $s; abstract function f(); public function __construct(private $p) {} }",
    );
    for what in [
        "interface declaration",
        "trait declaration",
        "enum declaration",
        "abstract class",
        "class constant",
        "static property",
        "abstract method",
        "constructor property promotion",
    ] {
        assert!(
            msgs.iter().any(|m| m.contains(what)),
            "missing {what:?} in {msgs:?}"
        );
    }
}

#[test]
fn nested_declarations_are_not_lowered() {
    let msgs = unsupported_messages("<?php if (1) { function f() {} class C {} }");
    assert!(
        msgs.iter()
            .any(|m| m.contains("nested function declaration")),
        "{msgs:?}"
    );
    assert!(
        msgs.iter().any(|m| m.contains("nested class declaration")),
        "{msgs:?}"
    );
}

#[test]
fn global_namespace_body_and_inline_html_lower() {
    let m = compile_ok("<?php namespace { echo 1; }");
    assert_eq!(
        m.func(0)
            .code
            .iter()
            .filter(|op| matches!(op, Op::Echo { .. }))
            .count(),
        1
    );
    let m = compile_ok("head\n<?php echo 1; ?>\ntail");
    let main = m.func(0);
    assert!(main.consts.contains(&Const::Str(Str::new(b"head\n"))));
    assert!(main.consts.contains(&Const::Str(Str::new(b"tail"))));
    assert_eq!(
        main.code
            .iter()
            .filter(|op| matches!(op, Op::Echo { .. }))
            .count(),
        3
    );
}

/// Every branch target must point at a valid instruction index.
fn assert_branch_targets_in_range(code: &[Op]) {
    let n = code.len() as CodeAddr;
    for op in code {
        match op {
            Op::Jmp { target } | Op::JmpIfTrue { target, .. } | Op::JmpIfFalse { target, .. } => {
                assert!(*target < n, "branch target {target} out of range (len {n})");
            }
            _ => {}
        }
    }
}
