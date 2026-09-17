//! Hoisting classification, verified against php 8.5.0 with
//! `class_exists("X", false)` / a call before the declaration.

mod common;

use rphp_hir::{class_hoist, ClassHoist};

fn names(v: &[&rphp_ast::v2::ClassLike], l: &common::Lowered) -> Vec<String> {
    v.iter().map(|c| l.text(c.name.unwrap())).collect()
}

/// php: `echo f(); { function f(){} }` works; `declare(ticks=1) { function f(){} }`
/// and `if (true) { function f(){} }` do not.
#[test]
fn functions_hoist_from_top_level_blocks_only() {
    let l = common::lower("<?php namespace N { function a(){} { function b(){} } if (1) { function d(){} } declare(ticks=1) { function e(){} } function outer() { function inner(){} } } namespace { function c(){} }");
    let h = l.hir.hoisted();
    // Declared names are FQNs after resolution.
    let funcs: Vec<String> = h.funcs.iter().map(|f| l.text(f.name)).collect();
    assert_eq!(funcs, ["N\\a", "N\\b", "N\\outer", "c"]);
}

/// php: `var_dump(new B instanceof A); class A {} class B extends A {}` → true
/// (B is early-bound because A is known); `class B extends Exception {}`
/// too; `class B implements Countable` → `class_exists("B", false)` is false;
/// `class B extends A { use T; }` false; `enum E {}` false (implicit
/// `UnitEnum`); `interface I extends J` false.
#[test]
fn class_classification() {
    let l = common::lower("<?php class A {} class B extends A {} class C implements Countable {} class D { use T; } enum E {} interface I {} interface J extends I {} trait T {} class F extends A implements I {} enum G: int implements I { case X = 1; } abstract class H {}");
    let h = l.hir.hoisted();
    assert_eq!(names(&h.classes, &l), ["A", "I", "T", "H"]);
    assert_eq!(names(&h.classes_try_early, &l), ["B"]);
    let kinds: Vec<ClassHoist> = l
        .program()
        .items
        .iter()
        .filter_map(|s| match s {
            rphp_ast::v2::Stmt::ClassLike(c) => Some(class_hoist(c)),
            _ => None,
        })
        .collect();
    assert_eq!(
        kinds,
        [
            ClassHoist::Early,
            ClassHoist::TryEarly,
            ClassHoist::InOrder,
            ClassHoist::InOrder,
            ClassHoist::InOrder,
            ClassHoist::Early,
            ClassHoist::InOrder,
            ClassHoist::Early,
            ClassHoist::InOrder,
            ClassHoist::InOrder,
            ClassHoist::Early,
        ]
    );
}

#[test]
fn nested_declarations_are_never_hoisted() {
    let l = common::lower(
        "<?php if (1) { class A {} } function f() { class B {} } $x = new class {}; { class C {} }",
    );
    let h = l.hir.hoisted();
    assert_eq!(names(&h.classes, &l), ["C"]);
    assert!(h.classes_try_early.is_empty());
}
