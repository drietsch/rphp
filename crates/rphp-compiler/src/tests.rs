//! Unit tests: every snippet goes through the front end (`parse_v2`) so the
//! tests exercise the tree shapes the adapter really produces.

use super::*;
use rphp_bytecode::{CodeAddr, Const, FnFlags, InitRef, Op};
use rphp_diagnostics::codes;
use rphp_parser::{parse_v2, ParseOptions};
use rphp_span::FileId;
use rphp_value::Str;

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
    compile(program, &mut interner, &CompileOptions::new())
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

fn count<F: Fn(&Op) -> bool>(code: &[Op], f: F) -> usize {
    code.iter().filter(|op| f(op)).count()
}

/// The function named `name` in the module.
fn func_named<'m>(m: &'m Module, name: &str) -> &'m rphp_bytecode::Function {
    m.funcs
        .iter()
        .find(|f| &*f.name_bytes == name.as_bytes())
        .unwrap_or_else(|| panic!("no function {name}"))
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
        line_of: Some(&line_of),
        file: None,
    };
    let m = compile(program, &mut interner, &opts).unwrap();
    let main = m.func(0);
    assert_eq!(main.lines.len(), main.code.len());
    // BindSymtab, then `echo 1` (2 ops) on line 1, `echo 2` on line 3, the
    // trailing `return 1` (2 ops) on line 3.
    assert_eq!(main.lines, vec![0, 1, 1, 3, 3, 3, 3]);
    // Without `line_of` the table stays empty (contract: "or empty").
    let m = compile_ok("<?php");
    assert!(m.func(0).lines.is_empty());
}

#[test]
fn main_binds_its_symtab_and_returns_one() {
    let m = compile_ok("<?php echo 1 + 2;");
    assert_eq!(m.main, 0);
    assert_eq!(m.funcs.len(), 1);
    let main = m.func(0);
    assert_eq!(main.num_params, 0);
    assert!(main.flags.contains(FnFlags::NEEDS_SYMTAB));
    assert_eq!(main.code[0], Op::BindSymtab);
    assert!(main.code.iter().any(|op| matches!(op, Op::Add { .. })));
    assert!(main.code.iter().any(|op| matches!(op, Op::Echo { .. })));
    assert!(matches!(main.code.last(), Some(Op::Ret { src: Some(_) })));
    assert_eq!(main.consts.last(), Some(&Const::Int(1)));
}

#[test]
fn variable_stores_go_through_assign_through_ref() {
    let m = compile_ok("<?php $x = 5; echo $x;");
    let main = m.func(0);
    // $x is register 0 (the only variable, no params) and is named.
    assert_eq!(main.var_names, vec![(Box::from(&b"x"[..]), 0)]);
    assert!(main.code.iter().any(|op| matches!(op, Op::Echo { src: 0 })));
    assert!(main
        .code
        .iter()
        .any(|op| matches!(op, Op::AssignThroughRef { dst: 0, .. })));
    // $x = $x + 1: the old value is read before the result lands.
    let m = compile_ok("<?php $x = $x + 1;");
    let add = m
        .func(0)
        .code
        .iter()
        .find_map(|op| match op {
            Op::Add { dst, a, b } => Some((*dst, *a, *b)),
            _ => None,
        })
        .expect("expected an Add");
    assert!(add.1 == 0 || add.2 == 0);
    assert_ne!(add.0, 0);
}

#[test]
fn calls_lower_to_init_send_docall_and_are_late_bound() {
    let m = compile_ok("<?php function foo($a) { return $a; } foo(7); bar(1, 2); \\strlen('x');");
    // main + foo; nothing is resolved at compile time.
    assert_eq!(m.funcs.len(), 2);
    assert_eq!(m.hoist_funcs, vec![1]);
    let foo = func_named(&m, "foo");
    assert_eq!(foo.num_params, 1);
    assert_eq!(&*foo.params[0].name, b"a");
    assert!(matches!(foo.code.first(), Some(Op::Ret { src: Some(0) })));
    let main = &m.func(0).code;
    assert_eq!(count(main, |op| matches!(op, Op::InitFCall { .. })), 3);
    assert_eq!(count(main, |op| matches!(op, Op::DoCall { .. })), 3);
    assert_eq!(count(main, |op| matches!(op, Op::SendVal { .. })), 4);
    let names: Vec<&[u8]> = m
        .func(0)
        .consts
        .iter()
        .filter_map(|c| c.as_name().map(|n| &*n.orig))
        .collect();
    assert_eq!(names, vec![&b"foo"[..], b"bar", b"strlen"]);
}

#[test]
fn argument_shapes_pick_the_send_form() {
    let m = compile_ok("<?php f($a, $a['k'], $o->p, g(), 1, ...$rest, name: $n);");
    let main = &m.func(0).code;
    assert_eq!(count(main, |op| matches!(op, Op::SendVar { pos: 0, .. })), 1);
    assert!(main.iter().any(|op| matches!(op, Op::SendRefElem { pos: 1, .. })));
    assert!(main.iter().any(|op| matches!(op, Op::SendRefProp { pos: 2, .. })));
    // A call result is sent as `SendFuncResult` (by value; php's notice when
    // the parameter is by-reference).
    assert!(main.iter().any(|op| matches!(op, Op::SendFuncResult { pos: 3, .. })));
    assert!(main.iter().any(|op| matches!(op, Op::SendVal { pos: 4, .. })));
    assert!(main.iter().any(|op| matches!(op, Op::SendUnpack { .. })));
    assert!(main.iter().any(|op| matches!(op, Op::SendNamed { .. })));
}

#[test]
fn parameter_defaults_variadics_and_by_ref() {
    let m = compile_ok("<?php function f($a, &$b, $c = 1, $d = [1, 2], ...$rest) {}");
    let f = func_named(&m, "f");
    assert!(f.flags.contains(FnFlags::VARIADIC));
    assert!(f.params[1].by_ref);
    assert!(matches!(f.params[2].default, Some(InitRef::Const(_))));
    // A non-literal default compiles to a thunk in the sink.
    let Some(InitRef::Thunk(t)) = f.params[3].default else {
        panic!("expected a thunk default");
    };
    assert!(m.func(t).code.iter().any(|op| matches!(op, Op::NewArray { .. })));
    assert!(f.params[4].variadic);
    assert_eq!(f.required_params(), 2);
    assert_eq!(count(&f.code, |op| matches!(op, Op::RecvInit { .. })), 2);
    assert!(f.code.iter().any(|op| matches!(op, Op::RecvVariadic { reg: 4 })));
}

#[test]
fn declarations_in_top_level_blocks_and_global_namespace_are_hoisted() {
    let m = compile_ok("<?php { function foo() {} } foo();");
    assert_eq!(m.hoist_funcs.len(), 1);
    let m = compile_ok("<?php namespace { { class C {} } function foo() {} foo(); new C; }");
    assert_eq!(m.funcs.len(), 2);
    assert_eq!(m.classes.len(), 1);
    assert_eq!(m.hoist_classes, vec![0]);
}

#[test]
fn conditional_declarations_are_declared_in_place() {
    let m = compile_ok("<?php if (!function_exists('f')) { function f() { return 1; } class K {} } echo f();");
    assert!(m.hoist_funcs.is_empty());
    assert!(m.hoist_classes.is_empty());
    let main = &m.func(0).code;
    assert!(main.iter().any(|op| matches!(op, Op::DeclareFunction { idx: 1 })));
    assert!(main.iter().any(|op| matches!(op, Op::DeclareClass { idx: 0 })));
    assert_eq!(&*func_named(&m, "f").name_bytes, b"f");
}

#[test]
fn duplicate_declarations_in_one_file_are_compile_errors() {
    // php: two unconditional declarations in one file are a compile-time
    // `Cannot redeclare` (the HIR pass reports it); a conditional second
    // declaration is left to the runtime.
    let diags = compile_err("<?php function foo() {} function foo() {}");
    assert!(has_code(&diags, codes::REDECLARED_FUNCTION), "{diags:#?}");
    let diags = compile_err("<?php class A {} class A {}");
    assert!(has_code(&diags, codes::REDECLARED_CLASS), "{diags:#?}");
    let m = compile_ok("<?php function foo() {} if (1) { function foo() {} }");
    assert_eq!(m.hoist_funcs.len(), 1);
    assert!(m.func(0).code.iter().any(|op| matches!(op, Op::DeclareFunction { .. })));
}

#[test]
fn logical_operators_short_circuit_with_branches() {
    let m = compile_ok("<?php echo true && false;");
    let code = &m.func(0).code;
    assert!(code.iter().any(|op| matches!(op, Op::JmpIfFalse { .. })));
    assert!(code.iter().any(|op| matches!(op, Op::Jmp { .. })));
    assert_eq!(count(code, |op| matches!(op, Op::Not { .. })), 2);
    assert_branch_targets_in_range(code);
    let m = compile_ok("<?php echo false || true; echo 1 xor 0;");
    let code = &m.func(0).code;
    assert!(code.iter().any(|op| matches!(op, Op::JmpIfTrue { .. })));
    assert!(code.iter().any(|op| matches!(op, Op::CmpNotIdentical { .. })));
    assert_branch_targets_in_range(code);
}

#[test]
fn if_elseif_while_do_for_lower_to_branches() {
    let m = compile_ok("<?php if (1) { echo 1; } elseif (2) { echo 2; } else { echo 3; }");
    let code = &m.func(0).code;
    assert_eq!(count(code, |op| matches!(op, Op::JmpIfFalse { .. })), 2);
    assert_eq!(count(code, |op| matches!(op, Op::Jmp { .. })), 2);
    assert_branch_targets_in_range(code);
    let m = compile_ok("<?php while (1) { echo 1; } do { echo 2; } while (0); for ($i = 0; $i < 3; $i++) { echo $i; }");
    let code = &m.func(0).code;
    let back_jmps = code.iter().enumerate().filter(|(i, op)| matches!(op, Op::Jmp { target } if (*target as usize) <= *i)).count();
    assert_eq!(back_jmps, 2, "while and for jump backwards");
    assert!(code.iter().any(|op| matches!(op, Op::JmpIfTrue { .. })), "do-while tests at the bottom");
    assert!(code.iter().any(|op| matches!(op, Op::IncDec { inc: true, pre: false, dst: None, .. })));
    assert_branch_targets_in_range(code);
}

#[test]
fn break_continue_levels_and_errors() {
    let m = compile_ok("<?php while (1) { for (;;) { if (1) break 2; continue 2; } }");
    assert_branch_targets_in_range(&m.func(0).code);
    // (A bare `break` outside a loop and a too-deep level are rejected by
    // the front end's conformance pass; the compiler keeps its own check as
    // defence in depth.)
}

#[test]
fn switch_and_match_use_jump_tables_for_literal_cases() {
    let m = compile_ok("<?php switch ($x) { case 1: echo 1; case 'a': echo 2; break; default: echo 3; }");
    let main = m.func(0);
    let sw = main
        .code
        .iter()
        .find_map(|op| match op {
            Op::Switch { table, strict, .. } => Some((*table, *strict)),
            _ => None,
        })
        .expect("switch");
    assert!(!sw.1);
    let rows = main.consts[sw.0 as usize].as_jump_table().unwrap();
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().all(|(_, t)| (*t as usize) < main.code.len()));
    // Non-literal cases compare in a chain.
    let m = compile_ok("<?php switch ($x) { case $y: echo 1; }");
    assert!(m.func(0).code.iter().any(|op| matches!(op, Op::CmpEq { .. })));
    // match is strict and throws without a default.
    let m = compile_ok("<?php echo match ($x) { 1, 2 => 'a', default => 'b' }; echo match ($x) { 3 => 'c' };");
    let code = &m.func(0).code;
    assert_eq!(count(code, |op| matches!(op, Op::Switch { strict: true, .. })), 2);
    assert_eq!(count(code, |op| matches!(op, Op::MatchError { .. })), 1);
    assert_branch_targets_in_range(code);
}

#[test]
fn goto_and_labels() {
    let m = compile_ok("<?php $i = 0; again: $i++; if ($i < 3) goto again; echo $i;");
    assert_branch_targets_in_range(&m.func(0).code);
    // `goto` validation happens in the HIR pass (php's compile-time texts).
    let diags = compile_err("<?php goto nope;");
    assert!(has_code(&diags, codes::UNDEFINED_LABEL), "{diags:#?}");
    let diags = compile_err("<?php goto inside; while (1) { inside: echo 1; }");
    assert!(has_code(&diags, codes::INVALID_JUMP), "{diags:#?}");
}

#[test]
fn string_literal_concat_and_interpolation() {
    let m = compile_ok("<?php echo \"hi\" . '!';");
    let main = m.func(0);
    assert!(main.consts.contains(&Const::Str(Str::new(b"hi"))));
    assert!(main.code.iter().any(|op| matches!(op, Op::Concat { .. })));
    // "a $x" => one ConcatN over two parts.
    let m = compile_ok("<?php echo \"a $x\";");
    let main = m.func(0);
    assert!(main.code.iter().any(|op| matches!(op, Op::ConcatN { n: 2, .. })));
    // A lone "$x" is still stringified.
    let m = compile_ok("<?php echo \"$x\";");
    assert!(m.func(0).code.iter().any(|op| matches!(op, Op::ConcatN { n: 1, .. })));
}

#[test]
fn array_literals_and_writes() {
    let m = compile_ok("<?php [1, 'k' => 2, &$r]; $a[] = 5; $a['x'][1] = 6; $o->p['k'] = 7; $b = &$a['z'];");
    let code = &m.func(0).code;
    assert!(code.iter().any(|op| matches!(op, Op::NewArray { .. })));
    assert!(code.iter().any(|op| matches!(op, Op::ArrayPush { .. })));
    assert!(code.iter().any(|op| matches!(op, Op::ArraySet { .. })));
    assert!(code.iter().any(|op| matches!(op, Op::AssignRefElem { .. })));
    // Nested writes: fetch-for-write then the write-back.
    assert!(code.iter().any(|op| matches!(op, Op::FetchElemW { .. })));
    assert!(code.iter().any(|op| matches!(op, Op::FetchPropW { .. })));
    assert!(code.iter().any(|op| matches!(op, Op::AssignProp { .. })));
    assert!(code.iter().any(|op| matches!(op, Op::RefElem { .. })));
    // (`f() = 1`, `$this = 1` and `$a[]` reads are rejected by the front
    // end's conformance pass; the compiler keeps `INVALID_WRITE_TARGET` /
    // `INVALID_APPEND_READ` as defence in depth.)
}

#[test]
fn foreach_lowers_to_iter_ops() {
    let m = compile_ok("<?php foreach ($a as $k => $v) { echo $v; } foreach ($b as &$r) {} foreach ($c as [$x, $y]) {}");
    let code = &m.func(0).code;
    assert_eq!(count(code, |op| matches!(op, Op::IterInit { by_ref: false, .. })), 2);
    assert_eq!(count(code, |op| matches!(op, Op::IterInit { by_ref: true, .. })), 1);
    assert_eq!(count(code, |op| matches!(op, Op::IterNext { .. })), 3);
    assert_eq!(count(code, |op| matches!(op, Op::IterFree { .. })), 3);
    // Destructuring reads elements 0 and 1 of the value temp.
    assert!(code.iter().any(|op| matches!(op, Op::ListGet { .. })));
    assert_branch_targets_in_range(code);
}

#[test]
fn references_globals_statics_and_symtab() {
    let m = compile_ok("<?php $a = &$b; global $g; static $s = 1; unset($a, $b['k'], $o->p); $$n = 1; echo $GLOBALS['x'];");
    let main = m.func(0);
    let code = &main.code;
    assert!(code.iter().any(|op| matches!(op, Op::AssignRef { .. })));
    assert!(code.iter().any(|op| matches!(op, Op::BindGlobal { .. })));
    assert!(code.iter().any(|op| matches!(op, Op::BindStatic { idx: 0, .. })));
    assert_eq!(main.statics.len(), 1);
    assert!(code.iter().any(|op| matches!(op, Op::UnsetVar { .. })));
    assert!(code.iter().any(|op| matches!(op, Op::UnsetElem { .. })));
    assert!(code.iter().any(|op| matches!(op, Op::UnsetProp { .. })));
    assert!(code.iter().any(|op| matches!(op, Op::BindDynVar { global: false, .. })));
    assert!(code.iter().any(|op| matches!(op, Op::FetchGlobals { .. })));
    // A function using compact() needs a symbol table; a plain one does not.
    let m = compile_ok("<?php function f($a) { return compact('a'); } function g($a) { return $a; }");
    assert!(func_named(&m, "f").flags.contains(FnFlags::NEEDS_SYMTAB));
    assert_eq!(func_named(&m, "f").code[0], Op::BindSymtab);
    assert!(!func_named(&m, "g").flags.contains(FnFlags::NEEDS_SYMTAB));
}

#[test]
fn operators_compound_assignment_and_casts() {
    let m = compile_ok("<?php $a += 1; $a .= 'x'; $a['k'] -= 2; $o->p *= 3; $a ??= 4; $b = $c ?? 5; $d = $e ? 1 : 2; $f = $g ?: 3; echo (int) $a, +$a, ~$a, $a << 1, $a & 2, $a ^ 3, $a | 4, $a >> 5;");
    let code = &m.func(0).code;
    assert!(code.iter().any(|op| matches!(op, Op::AssignOp { op: rphp_bytecode::AssignOpKind::Add, .. })));
    assert!(code.iter().any(|op| matches!(op, Op::AssignOp { op: rphp_bytecode::AssignOpKind::Concat, .. })));
    assert!(code.iter().any(|op| matches!(op, Op::AssignOpElem { .. })));
    assert!(code.iter().any(|op| matches!(op, Op::AssignOpProp { .. })));
    assert!(code.iter().any(|op| matches!(op, Op::IssetVar { .. })));
    assert!(code.iter().any(|op| matches!(op, Op::ArrayGetQuiet { .. })) || code.iter().any(|op| matches!(op, Op::IssetVar { .. })));
    assert!(code.iter().any(|op| matches!(op, Op::Cast { kind: rphp_bytecode::CastKind::Int, .. })));
    for probe in [
        |op: &Op| matches!(op, Op::Plus { .. }),
        |op: &Op| matches!(op, Op::BitNot { .. }),
        |op: &Op| matches!(op, Op::Shl { .. }),
        |op: &Op| matches!(op, Op::BitAnd { .. }),
        |op: &Op| matches!(op, Op::BitXor { .. }),
        |op: &Op| matches!(op, Op::BitOr { .. }),
        |op: &Op| matches!(op, Op::Shr { .. }),
    ] {
        assert!(code.iter().any(probe));
    }
    assert_branch_targets_in_range(code);
}

#[test]
fn isset_empty_and_nullsafe_chains() {
    let m = compile_ok("<?php isset($a, $b['k']['j'], $o->p); empty($a); empty($b['k']); echo $o?->p?->q; $o?->m();");
    let code = &m.func(0).code;
    assert!(code.iter().any(|op| matches!(op, Op::IssetVar { .. })));
    assert!(code.iter().any(|op| matches!(op, Op::IssetElem { .. })));
    assert!(code.iter().any(|op| matches!(op, Op::IssetProp { .. })));
    assert!(code.iter().any(|op| matches!(op, Op::EmptyVar { .. })));
    assert!(code.iter().any(|op| matches!(op, Op::EmptyElem { .. })));
    assert!(code.iter().any(|op| matches!(op, Op::ArrayGetQuiet { .. })));
    assert!(code.iter().any(|op| matches!(op, Op::InitMethodCall { .. })));
    assert_branch_targets_in_range(code);
}

#[test]
fn closures_and_arrow_functions_capture() {
    let m = compile_ok(
        "<?php $a = 1; $b = 2; $f = function ($x) use ($a, &$b) { return $x + $a + $b; }; $g = fn($y) => $y * $b; $h = static fn() => 1;",
    );
    // main, then the three closures.
    assert_eq!(m.funcs.len(), 4);
    let main = m.func(0);
    assert_eq!(count(&main.code, |op| matches!(op, Op::MakeClosure { .. })), 3);
    let f = m.func(1);
    assert!(f.flags.contains(FnFlags::CLOSURE));
    assert_eq!(f.captures.len(), 2);
    assert!(!f.captures[0].by_ref);
    assert!(f.captures[1].by_ref);
    assert_eq!(f.captures[0].dst, 1); // after the parameter
    assert!(f.name_bytes.starts_with(b"{closure:"));
    let g = m.func(2);
    assert_eq!(g.captures.len(), 1);
    assert!(g.code.iter().any(|op| matches!(op, Op::Ret { src: Some(_) })));
    assert!(m.func(3).flags.contains(FnFlags::STATIC));
    // The inner arrow function reads $z; the outer one must capture it too.
    let m = compile_ok("<?php $z = 1; $f = fn($a) => fn($b) => $a + $b + $z;");
    let outer = m.func(1);
    assert_eq!(outer.captures.len(), 1);
    assert_eq!(outer.captures[0].src, 0);
}

#[test]
fn class_compiles_methods_with_this_as_a_frame_slot() {
    let m = compile_ok(
        "<?php class C { public $v = 1; public $arr = [1, 'a' => 2]; function get() { return $this->v; } }\n\
         $c = new C(); $c->get(); $c->v = 2; $c instanceof C;",
    );
    assert_eq!(m.classes.len(), 1);
    assert_eq!(m.classes[0].props.len(), 2);
    assert_eq!(m.classes[0].props[0].default, rphp_value::Value::Int(1));
    assert!(matches!(m.classes[0].props[1].default, rphp_value::Value::Array(_)));
    assert_eq!(m.classes[0].methods.len(), 1);
    let main = &m.func(0).code;
    assert!(main.iter().any(|op| matches!(op, Op::InitNew { .. })));
    assert!(main.iter().any(|op| matches!(op, Op::InitMethodCall { .. })));
    assert!(main.iter().any(|op| matches!(op, Op::AssignProp { .. })));
    assert!(main.iter().any(|op| matches!(op, Op::InstanceOfRef { .. })));
    let method = m.func(m.classes[0].methods[0].func);
    assert_eq!(method.num_params, 0, "$this is not a parameter");
    assert!(method.flags.contains(FnFlags::USES_THIS));
    assert!(method.code.iter().any(|op| matches!(op, Op::LoadThis { .. })));
    assert!(method.code.iter().any(|op| matches!(op, Op::FetchProp { .. })));
    assert_eq!(method.in_class, Some(0));
}

#[test]
fn scoped_calls_and_class_names() {
    let m = compile_ok(
        "<?php class A { function m() { return 1; } }\n\
         class B extends A { function m() { return parent::m() + self::m() + static::m(); } function n() { return [self::class, static::class, B::class, $this::class]; } }\n\
         A::m(); $x::m(); new $cls; new (trim(' A ')); $o instanceof $cls;",
    );
    let b_m = m.func(m.classes[1].methods[0].func);
    assert_eq!(count(&b_m.code, |op| matches!(op, Op::InitStaticCall { .. })), 3);
    let b_n = m.func(m.classes[1].methods[1].func);
    assert_eq!(count(&b_n.code, |op| matches!(op, Op::FetchClassConst { .. })), 2);
    assert!(b_n.consts.contains(&Const::Str(Str::new(b"B"))));
    let main = &m.func(0).code;
    assert_eq!(count(main, |op| matches!(op, Op::InitStaticCall { .. })), 2);
    assert_eq!(count(main, |op| matches!(op, Op::InitNew { .. })), 2);
    assert!(main.iter().any(|op| matches!(op, Op::InstanceOfRef { .. })));
    let diags = compile_err("<?php self::m();");
    assert!(has_code(&diags, INVALID_SCOPE));
}

#[test]
fn constants_and_magic_constants() {
    let src = "<?php const X = 1; echo X, PHP_EOL, __LINE__, __FILE__, __DIR__, __FUNCTION__;\nfunction f() { return [__FUNCTION__, __METHOD__]; }";
    let mut interner = Interner::new();
    let program = parse(src, &mut interner);
    let line_of = |off: u32| 1 + src.as_bytes()[..off as usize].iter().filter(|&&b| b == b'\n').count() as u32;
    let opts = CompileOptions {
        line_of: Some(&line_of),
        file: Some(PathBuf::from("/tmp/dir/file.php")),
    };
    let m = compile(program, &mut interner, &opts).unwrap();
    let main = m.func(0);
    assert!(main.code.iter().any(|op| matches!(op, Op::DeclareConst { .. })));
    assert_eq!(count(&main.code, |op| matches!(op, Op::FetchConst { .. })), 2);
    assert!(main.consts.contains(&Const::Str(Str::new(b"/tmp/dir/file.php"))));
    assert!(main.consts.contains(&Const::Str(Str::new(b"/tmp/dir"))));
    assert!(main.consts.contains(&Const::Int(1)), "__LINE__");
    assert_eq!(&*m.file, "/tmp/dir/file.php");
    let f = func_named(&m, "f");
    assert!(f.consts.contains(&Const::Str(Str::new(b"f"))));
}

#[test]
fn misc_expressions_lower() {
    let m = compile_ok("<?php print 1; exit(3); @f(); throw $e; clone $o; include 'x.php'; require_once $p; $a = [$b, [$c, $d]] = $e; list('k' => $z) = $e; (void) g();");
    let code = &m.func(0).code;
    assert!(code.iter().any(|op| matches!(op, Op::Exit { src: Some(_) })));
    assert_eq!(count(code, |op| matches!(op, Op::Silence { .. })), 2);
    assert!(code.iter().any(|op| matches!(op, Op::Throw { .. })));
    assert!(code.iter().any(|op| matches!(op, Op::Clone { .. })));
    assert_eq!(count(code, |op| matches!(op, Op::Include { .. })), 2);
    assert!(m.func(0).flags.contains(FnFlags::NEEDS_SYMTAB));
    assert!(code.iter().any(|op| matches!(op, Op::ListGet { .. })));
}

#[test]
fn unsupported_constructs_report_e0300_with_a_description() {
    // E6 lowered class declarations, static access and first-class
    // callables; E7 lowered `eval`; E8 lowered `yield`. What is left is the
    // genuinely unlowered tail.
    let msgs = unsupported_messages("<?php enum E: string { case A = 1 << 0; }");
    assert!(msgs.iter().any(|m| m.contains("non-literal enum case value")), "{msgs:?}");
    assert!(msgs
        .iter()
        .all(|m| m.starts_with("unsupported construct: ") && m.ends_with(" (not lowered yet)")));

    // `unset(C::$p)` and `C::$p = &$x` lower as of the P4 trunk: php resolves
    // both at run time, so the compiler only has to emit the op.
    compile_ok("<?php class C { public static $p; } unset(C::$p); $x = 1; C::$p = &$x;");
}

#[test]
fn class_declarations_and_static_access_lower_after_e6() {
    // The counterpart of the removed E0300 cases: these must now compile.
    let m = compile_ok(
        "<?php interface I {} trait T {} enum E {} abstract class A { const K = 1; static $s;\n\
         abstract function f(); public function __construct(private $p) {} static function s() {} }",
    );
    assert_eq!(m.classes.len(), 4, "interface, trait, enum and class all lower");
    compile_ok("<?php class B { public static $p; const K = 1; } B::$p; B::K; strlen(...);");
}

#[test]
fn global_namespace_body_and_inline_html_lower() {
    let m = compile_ok("<?php namespace { echo 1; }");
    assert_eq!(count(&m.func(0).code, |op| matches!(op, Op::Echo { .. })), 1);
    let m = compile_ok("head\n<?php echo 1; ?>\ntail");
    let main = m.func(0);
    assert!(main.consts.contains(&Const::Str(Str::new(b"head\n"))));
    assert!(main.consts.contains(&Const::Str(Str::new(b"tail"))));
    assert_eq!(count(&main.code, |op| matches!(op, Op::Echo { .. })), 3);
}

/// Names come from the resolver: declarations register under their FQN,
/// class positions carry the static FQN, and an unqualified function or
/// constant inside a namespace carries both candidates of php's two-step
/// lookup (`name` = the namespaced one, `ns_fallback` = the global one).
#[test]
fn namespaced_names_are_resolved_and_two_step_where_php_says_so() {
    let m = compile_ok(
        "<?php namespace App\\Models; use Other\\Thing as T; use function Other\\helper; use const Other\\LIMIT;\n\
         const X = 1; function f() {} class C { function m() { return [__CLASS__, __METHOD__, __FUNCTION__, __NAMESPACE__, self::class]; } }\n\
         new C; new T; new \\Exception; new namespace\\D; f(); \\strlen('a'); strlen('a'); helper(); Sub\\g();\n\
         echo X, \\PHP_EOL, PHP_EOL, LIMIT, namespace\\Y, C::class, T::class;",
    );
    assert_eq!(&*func_named(&m, "App\\Models\\f").name_bytes, b"App\\Models\\f");
    assert_eq!(&*m.classes[0].name_bytes, b"App\\Models\\C");
    let main = m.func(0);
    let name = |k: u32| match &main.consts[k as usize] {
        Const::Name(n) => String::from_utf8_lossy(&n.orig).into_owned(),
        other => panic!("not a name: {other:?}"),
    };
    let news: Vec<String> = main
        .code
        .iter()
        .filter_map(|op| match op {
            Op::InitNew { class, .. } => match class.kind() {
                rphp_bytecode::ClassRefKind::Named(i) => Some(name(i)),
                _ => None,
            },
            _ => None,
        })
        .collect();
    assert_eq!(
        news,
        ["App\\Models\\C", "Other\\Thing", "Exception", "App\\Models\\D"]
    );
    let calls: Vec<(String, Option<String>)> = main
        .code
        .iter()
        .filter_map(|op| match op {
            Op::InitFCall {
                name: n,
                ns_fallback,
                ..
            } => Some((name(*n), ns_fallback.map(name))),
            _ => None,
        })
        .collect();
    assert_eq!(
        calls,
        [
            ("App\\Models\\f".to_string(), Some("f".to_string())),
            ("strlen".to_string(), None),
            ("App\\Models\\strlen".to_string(), Some("strlen".to_string())),
            ("Other\\helper".to_string(), None),
            ("App\\Models\\Sub\\g".to_string(), None),
        ]
    );
    let consts: Vec<(String, Option<String>)> = main
        .code
        .iter()
        .filter_map(|op| match op {
            Op::FetchConst {
                name: n,
                ns_fallback,
                ..
            } => Some((name(*n), ns_fallback.map(name))),
            _ => None,
        })
        .collect();
    assert_eq!(
        consts,
        [
            ("App\\Models\\X".to_string(), Some("X".to_string())),
            ("PHP_EOL".to_string(), None),
            ("App\\Models\\PHP_EOL".to_string(), Some("PHP_EOL".to_string())),
            ("Other\\LIMIT".to_string(), None),
            ("App\\Models\\Y".to_string(), None),
        ]
    );
    // `const X` declares `App\Models\X`; `::class` and the magic constants
    // are folded strings.
    let declared: Vec<String> = main
        .code
        .iter()
        .filter_map(|op| match op {
            Op::DeclareConst { name: n, .. } => Some(name(*n)),
            _ => None,
        })
        .collect();
    assert_eq!(declared, ["App\\Models\\X"]);
    for folded in ["App\\Models\\C", "Other\\Thing"] {
        assert!(main.consts.contains(&Const::Str(Str::new(folded.as_bytes()))), "{folded}");
    }
    let method = m.func(m.classes[0].methods[0].func);
    for folded in ["App\\Models\\C", "App\\Models\\C::m", "m", "App\\Models"] {
        assert!(method.consts.contains(&Const::Str(Str::new(folded.as_bytes()))), "{folded}");
    }
    assert_eq!(count(&main.code, |op| matches!(op, Op::FetchClass { .. })), 0);
}

/// `use` is inert and every namespace form is a transparent statement list;
/// declarations inside named namespace bodies are still hoisted.
#[test]
fn namespace_bodies_are_transparent_and_use_is_inert() {
    let m = compile_ok("<?php namespace A { use B\\C; function f() {} class K {} echo 1; } namespace D { function g() {} echo 2; }");
    assert_eq!(m.hoist_funcs.len(), 2);
    assert_eq!(m.hoist_classes, vec![0]);
    assert_eq!(count(&m.func(0).code, |op| matches!(op, Op::Echo { .. })), 2);
    assert_eq!(&*func_named(&m, "A\\f").name_bytes, b"A\\f");
    assert_eq!(&*func_named(&m, "D\\g").name_bytes, b"D\\g");
    // php's compile-time fatal for mixed namespace forms is an HIR error.
    let diags = compile_err("<?php namespace A; use B\\C; use D\\C;");
    assert!(has_code(&diags, codes::IMPORT_CONFLICT), "{diags:#?}");
}

/// The HIR's `Let`/`Temp`/`Seq` shapes: destructuring reads its snapshot
/// with `ListGet`, `?:` evaluates its operand once, a stabilized `??=`
/// evaluates the key once, a nullsafe chain is an identical-to-null test,
/// and a destructuring `foreach` binds its temporary.
#[test]
fn hir_temporaries_lower_to_registers() {
    let m = compile_ok("<?php [$a, [$b, $c]] = f(); ['k' => $d] = f2(); $e = g() ?: 0; $h[k()] ??= 1; $n = $o?->p?->q(); foreach (xs() as [$x, $y]) { echo $x; }");
    let code = &m.func(0).code;
    assert_eq!(count(code, |op| matches!(op, Op::ListGet { .. })), 7);
    assert_eq!(count(code, |op| matches!(op, Op::InitFCall { .. })), 5, "every call once");
    assert!(code.iter().any(|op| matches!(op, Op::CmpIdentical { .. })));
    assert!(code.iter().any(|op| matches!(op, Op::IterNext { .. })));
    // One snapshot per `Let`: the two patterns (outer + nested), `?:`, the
    // stabilized key, the nullsafe chain (two links).
    assert_eq!(count(code, |op| matches!(op, Op::Deref { .. })), 7, "one snapshot per Let");
    assert_branch_targets_in_range(code);
    // A by-reference pattern reads from the stabilized source itself.
    let m = compile_ok("<?php [$a, &$b] = $arr; foreach ($rows as [$k, &$v]) { $v++; }");
    let code = &m.func(0).code;
    assert_eq!(count(code, |op| matches!(op, Op::RefElem { .. })), 2);
    assert!(code.iter().any(|op| matches!(op, Op::IterInit { by_ref: true, .. })));
    assert_branch_targets_in_range(code);
}

#[test]
fn strict_types_marks_every_function() {
    let m = compile_ok("<?php declare(strict_types=1); function f() {}");
    assert!(m.func(0).flags.contains(FnFlags::STRICT_TYPES));
    assert!(func_named(&m, "f").flags.contains(FnFlags::STRICT_TYPES));
}

/// Every branch target must point at a valid instruction index.
fn assert_branch_targets_in_range(code: &[Op]) {
    let n = code.len() as CodeAddr;
    for op in code {
        match op {
            Op::Jmp { target }
            | Op::JmpIfTrue { target, .. }
            | Op::JmpIfFalse { target, .. }
            | Op::IterNext { target, .. }
            | Op::Switch {
                default: target, ..
            } => {
                assert!(*target < n, "branch target {target} out of range (len {n})");
            }
            _ => {}
        }
    }
}
