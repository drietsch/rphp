//! `goto`/label, `break`/`continue`, `const`-in-function and promotion
//! validation, with the `php -n -r` one-liners (php 8.5.0) that fixed each
//! message.

mod common;

use common::{lower, lower_lenient};

fn hir_errors(src: &str) -> Vec<String> {
    lower_lenient(src).0.errors()
}

/// php: `goto x;` → `'goto' to undefined label 'x'`; labels are per
/// function body: `function f() { goto x; } x:` and `goto x; function f() { x: }`
/// both fail, and so does a closure (`$f = function() { goto x; }; x:`).
#[test]
fn undefined_labels_are_per_body() {
    assert_eq!(
        hir_errors("<?php goto x;"),
        ["RPHP_E0202 'goto' to undefined label 'x'"]
    );
    assert_eq!(
        hir_errors("<?php function f() { goto x; } x:"),
        ["RPHP_E0202 'goto' to undefined label 'x'"]
    );
    assert_eq!(
        hir_errors("<?php goto x; function f() { x: }"),
        ["RPHP_E0202 'goto' to undefined label 'x'"]
    );
    assert_eq!(
        hir_errors("<?php $f = function() { goto x; }; x:"),
        ["RPHP_E0202 'goto' to undefined label 'x'"]
    );
    assert_eq!(
        hir_errors("<?php goto x; class A { function m() { x: } }"),
        ["RPHP_E0202 'goto' to undefined label 'x'"]
    );
    assert!(hir_errors("<?php goto x; x: function f() { goto y; y: }").is_empty());
}

/// php: `x: x:` → `Label 'x' already defined`.
#[test]
fn duplicate_label() {
    assert_eq!(
        hir_errors("<?php x: x:"),
        ["RPHP_E0202 Label 'x' already defined"]
    );
}

/// php: `goto x; while(1) { x: }`, `for`, `foreach`, `do`, `switch`,
/// `if (1) { while (1) { x: } }` → `'goto' into loop or switch statement is
/// disallowed`; jumping *out* (`while (1) { goto x; } x:`), into an `if`
/// (`goto x; if (1) { x: }`), into a block, or within the same loop
/// (`while (1) { x: goto x; }`) is fine.
#[test]
fn goto_into_loops() {
    for src in [
        "<?php goto x; while(1) { x: }",
        "<?php goto x; for (;;) { x: }",
        "<?php goto x; foreach ([] as $v) { x: }",
        "<?php goto x; do { x: } while (0);",
        "<?php goto x; switch (1) { case 1: { x: } }",
        "<?php goto x; if (1) { while (1) { x: } }",
        "<?php goto x; while (1) { while (1) { x: } }",
        "<?php while(1) { goto x; while(1) { x: } }",
    ] {
        assert_eq!(
            hir_errors(src),
            ["RPHP_E0203 'goto' into loop or switch statement is disallowed"],
            "{src}"
        );
    }
    for src in [
        "<?php while (1) { goto x; } x:",
        "<?php goto x; if (1) { x: }",
        "<?php goto x; { x: }",
        "<?php while (1) { x: goto x; }",
        "<?php x: while (1) { goto x; }",
        "<?php goto x; while(1) { } x:",
    ] {
        assert!(hir_errors(src).is_empty(), "{src}");
    }
}

/// php: `try {} finally { goto x; } x:` → `jump out of a finally block is disallowed`
/// (also from a nested `try` inside the finally, and to a label in the
/// enclosing `try` body); `goto x; try {} finally { x: }` → `jump into a
/// finally block is disallowed`; jumps within one finally, out of a `try`
/// body or a `catch` are fine.
#[test]
fn goto_and_finally() {
    for src in [
        "<?php try {} finally { goto x; } x:",
        "<?php try {} finally { try { goto x; } finally { } } x:",
        "<?php try { try {} finally { goto x; } x: } finally {}",
        "<?php try {} finally { goto x; } try {} finally { x: }",
    ] {
        assert_eq!(
            hir_errors(src),
            ["RPHP_E0203 jump out of a finally block is disallowed"],
            "{src}"
        );
    }
    assert_eq!(
        hir_errors("<?php goto x; try {} finally { x: }"),
        ["RPHP_E0203 jump into a finally block is disallowed"]
    );
    for src in [
        "<?php try { goto x; } catch (E) {} x:",
        "<?php try {} catch (E) { goto x; } x:",
        "<?php try {} finally { goto x; x: }",
        "<?php try {} finally { if (1) { goto x; } x: }",
        "<?php try {} finally { try { goto x; x: } finally {} }",
        "<?php goto x; try {} finally { } x:",
        "<?php goto x; try { x: } finally {}",
    ] {
        assert!(hir_errors(src).is_empty(), "{src}");
    }
}

/// php: `break;` → `'break' not in the 'loop' or 'switch' context`;
/// `while(1) { break 2; }` → `Cannot 'break' 2 levels`;
/// `while(1) { while (1) { continue 3; } }` → `Cannot 'continue' 3 levels`.
/// (The adapter reports these too, as `RPHP_E0011`.)
#[test]
fn break_continue_levels() {
    assert_eq!(
        hir_errors("<?php break;"),
        ["RPHP_E0203 'break' not in the 'loop' or 'switch' context"]
    );
    assert_eq!(
        hir_errors("<?php while(1) { break 2; }"),
        ["RPHP_E0203 Cannot 'break' 2 levels"]
    );
    assert_eq!(
        hir_errors("<?php while(1) { while (1) { continue 3; } }"),
        ["RPHP_E0203 Cannot 'continue' 3 levels"]
    );
    assert!(hir_errors(
        "<?php while(1) { switch (1) { case 1: continue 2; } for (;;) { break 2; } }"
    )
    .is_empty());
    // A closure body starts its own loop context.
    assert_eq!(
        hir_errors("<?php while (1) { $f = function() { break; }; }"),
        ["RPHP_E0203 'break' not in the 'loop' or 'switch' context"]
    );
}

/// php: `function f(){ const X = 1; }` → `syntax error, unexpected token "const"`.
#[test]
fn const_in_function_body() {
    let (l, parse) = lower_lenient("<?php function f(){ const X = 1; }");
    // mago accepts it; php does not.
    assert!(parse.is_empty() || parse.iter().all(|d| d.code == "RPHP_E0011"));
    assert_eq!(
        l.errors(),
        ["RPHP_E0011 syntax error, unexpected token \"const\""]
    );
    assert!(
        lower("<?php namespace N { const Y = 2; } namespace { const X = 1; }")
            .errors()
            .is_empty()
    );
}

/// php: `class A { function m(public $x) {} }` → `Cannot declare promoted property outside a constructor`;
/// `function f(public $x) {}` and closures likewise;
/// `class A { function __construct(public ...$x) {} }` → `Cannot declare variadic promoted property`.
#[test]
fn promotion_placement() {
    assert_eq!(
        hir_errors("<?php class A { function m(public $x) {} }"),
        ["RPHP_E0011 Cannot declare promoted property outside a constructor"]
    );
    assert_eq!(
        hir_errors("<?php $c = function(private $x) {};"),
        ["RPHP_E0011 Cannot declare promoted property outside a constructor"]
    );
    assert_eq!(
        hir_errors("<?php class A { function __construct(public ...$x) {} }"),
        ["RPHP_E0011 Cannot declare variadic promoted property"]
    );
    assert!(lower(
        "<?php class A { function __CONSTRUCT(public $x, protected(set) readonly int $y = 1) {} }"
    )
    .errors()
    .is_empty());
}

/// PHP 8.3: static variable initializers may be any expression.
#[test]
fn static_initializers_accept_expressions() {
    assert!(
        lower("<?php function f() { static $x = f() + 1, $y = new A, $z; }")
            .errors()
            .is_empty()
    );
}
