//! Name-resolution cases mirroring php-src's `Zend/tests/ns_0*.phpt`
//! outcomes. Each case names the `php -n -r` one-liner (php 8.5.0) whose
//! output established the expectation: `X::class` prints the resolved
//! class name, an undefined function/constant error prints the name php
//! looked up.

mod common;

use common::{lower, lower_lenient};
use rphp_diagnostics::codes;

fn classes(src: &str) -> Vec<String> {
    lower(src)
        .class_names()
        .into_iter()
        .map(|(f, _)| f)
        .collect()
}

fn keys(src: &str) -> Vec<String> {
    lower(src)
        .class_names()
        .into_iter()
        .map(|(_, k)| k)
        .collect()
}

fn funcs(src: &str) -> Vec<(Option<String>, String)> {
    lower(src).func_names()
}

fn consts(src: &str) -> Vec<(Option<String>, String)> {
    lower(src).const_names()
}

fn s(v: &[(&str, &str)]) -> Vec<(Option<String>, String)> {
    v.iter()
        .map(|(a, b)| {
            (
                if a.is_empty() {
                    None
                } else {
                    Some((*a).to_owned())
                },
                (*b).to_owned(),
            )
        })
        .collect()
}

// ----- classes (ns_001..ns_020 family) ------------------------------------------

/// php: `namespace A\B; use C\D; echo D::class, "|", D\E::class, "|", d::class;`
/// → `C\D|C\D\E|C\D`
#[test]
fn ns_unqualified_import_and_qualified_first_segment() {
    assert_eq!(
        classes("<?php namespace A\\B; use C\\D; new D; new D\\E; new d;"),
        ["C\\D", "C\\D\\E", "C\\D"]
    );
}

/// php: `namespace A\B; use C\D as F; echo F::class, "|", f::class, "|", F\G::class;`
/// → `C\D|C\D|C\D\G`
#[test]
fn ns_alias_is_case_insensitive() {
    assert_eq!(
        classes("<?php namespace A\\B; use C\\D as F; new F; new f; new F\\G;"),
        ["C\\D", "C\\D", "C\\D\\G"]
    );
}

/// php: `namespace A\B; echo X::class, "|", X\Y::class, "|", \X::class, "|", namespace\X::class;`
/// → `A\B\X|A\B\X\Y|X|A\B\X`
#[test]
fn ns_prefixing_without_imports() {
    assert_eq!(
        classes("<?php namespace A\\B; new X; new X\\Y; new \\X; new namespace\\X;"),
        ["A\\B\\X", "A\\B\\X\\Y", "X", "A\\B\\X"]
    );
}

/// php: `echo X::class, "|", X\Y::class, "|", \X\Y::class, "|", namespace\X::class;`
/// → `X|X\Y|X\Y|X`
#[test]
fn global_namespace_names() {
    assert_eq!(
        classes("<?php new X; new X\\Y; new \\X\\Y; new namespace\\X;"),
        ["X", "X\\Y", "X\\Y", "X"]
    );
}

/// php: `namespace A; use B\{C, D as E, F\G}; echo C::class, E::class, G::class, C\H::class;`
/// → `B\CB\DB\F\GB\C\H`
#[test]
fn group_use_classes() {
    assert_eq!(
        classes("<?php namespace A; use B\\{C, D as E, F\\G}; new C; new E; new G; new C\\H;"),
        ["B\\C", "B\\D", "B\\F\\G", "B\\C\\H"]
    );
}

#[test]
fn class_keys_are_lowercased_fqns() {
    assert_eq!(
        keys("<?php namespace Foo\\Bar; use Baz\\Qux as QQ; new QQ; new Zed; new \\Std\\Class_;"),
        ["baz\\qux", "foo\\bar\\zed", "std\\class_"]
    );
}

/// php: `namespace A { use B\C; } namespace D { echo C::class; }` → `D\C`:
/// imports do not leak across namespace blocks.
#[test]
fn imports_are_per_namespace_block() {
    assert_eq!(
        classes("<?php namespace A { use B\\C; new C; } namespace D { new C; }"),
        ["B\\C", "D\\C"]
    );
    // php: `namespace A; use B\C; namespace D; echo C::class;` → `D\C`
    assert_eq!(
        classes("<?php namespace A; use B\\C; new C; namespace D; new C;"),
        ["B\\C", "D\\C"]
    );
}

/// php (ReflectionFunction): `namespace A; use B\C; function f(C $x): ?C {}`
/// → parameter type `B\C`, return type `?B\C`.
#[test]
fn type_hints_resolve_as_classes() {
    assert_eq!(
        classes("<?php namespace A; use B\\C; function f(C $x, int|C $y): ?C { return null; }"),
        ["B\\C", "B\\C", "B\\C"]
    );
}

/// php (ReflectionClass): `namespace A; use B\C; #[C] class K {}` → attribute `B\C`.
#[test]
fn attributes_resolve_as_classes() {
    assert_eq!(
        classes("<?php namespace A; use B\\C; #[C, \\D, E] class K {}"),
        ["B\\C", "D", "A\\E"]
    );
}

/// php: every class position goes through the same rule — `catch`,
/// `extends`, `implements`, `use`, adaptations, `new`, `::`, `instanceof`
/// all report `B\C` when it is missing.
#[test]
fn every_class_position_resolves() {
    let src = "<?php namespace A; use B\\C;
        try {} catch (C | \\E $e) {}
        class K extends C implements C { use C, D { C::m insteadof D; D::m as n; } }
        interface I extends C {}
        enum En implements C { case X; }
        new C; C::f(); echo C::$p, C::X; var_dump(null instanceof C, null instanceof c);
        $x = new class extends C {};";
    assert_eq!(
        classes(src),
        [
            "B\\C", "E", // catch
            "B\\C", "B\\C", "B\\C", "A\\D", "B\\C", "A\\D", "A\\D", // class K
            "B\\C", // interface
            "B\\C", // enum
            "B\\C", "B\\C", "B\\C", "B\\C", "B\\C", "B\\C", // expressions
            "B\\C", // anonymous class
        ]
    );
}

/// php: `namespace A; use B\C; use function B\C; use const B\C; echo C::class;`
/// → `B\C` — the three tables are independent (and `c()` / `C` resolve
/// through their own).
#[test]
fn import_tables_are_independent() {
    let l = lower(
        "<?php namespace A; use B\\C; use function B\\C; use const B\\C; new C; c(); echo C;",
    );
    assert!(l.diags.is_empty(), "{:?}", l.messages());
    assert_eq!(l.class_names()[0].0, "B\\C");
    assert_eq!(l.func_names(), s(&[("", "b\\c")]));
    assert_eq!(l.const_names(), s(&[("", "b\\C")]));
}

// ----- functions (ns_022..ns_040 family) --------------------------------------------

/// php: `namespace A; use B\{function f, const X, C}; f();`
/// → `Call to undefined function B\f()`
#[test]
fn group_use_function_and_const() {
    assert_eq!(
        funcs("<?php namespace A; use B\\{function f, const X, C}; f();"),
        s(&[("", "b\\f")])
    );
    // php: `... echo X;` → `Undefined constant "B\X"`
    assert_eq!(
        consts("<?php namespace A; use B\\{function f, const X, C}; echo X;"),
        s(&[("", "b\\X")])
    );
}

/// php: `namespace A; use function B\f as g; G();` → `Call to undefined function B\f()`
#[test]
fn function_import_alias_case_insensitive() {
    assert_eq!(
        funcs("<?php namespace A; use function B\\f as g; G(); g();"),
        s(&[("", "b\\f"), ("", "b\\f")])
    );
}

/// php: `namespace A; f();` → `Call to undefined function A\f()` (after the
/// global fallback): the two-step keys.
#[test]
fn unqualified_function_in_namespace_is_two_step() {
    assert_eq!(
        funcs("<?php namespace A; f(); strlen('x');"),
        s(&[("a\\f", "f"), ("a\\strlen", "strlen")])
    );
}

/// php: `f();` → `Call to undefined function f()`
#[test]
fn unqualified_function_at_global_scope_is_single_step() {
    assert_eq!(
        funcs("<?php f(); STRLEN('x');"),
        s(&[("", "f"), ("", "strlen")])
    );
}

/// php: `namespace A; \f();` → `f()`; `namespace\f();` → `A\f()`;
/// `use B\C; C\f();` / `c\f();` → `B\C\f()`; `B\f();` → `A\B\f()`.
#[test]
fn qualified_function_names() {
    assert_eq!(
        funcs("<?php namespace A; use B\\C; \\f(); namespace\\f(); C\\f(); c\\f(); B\\f();"),
        s(&[
            ("", "f"),
            ("", "a\\f"),
            ("", "b\\c\\f"),
            ("", "b\\c\\f"),
            ("", "a\\b\\f")
        ])
    );
}

#[test]
fn function_import_beats_namespace_fallback() {
    assert_eq!(
        funcs("<?php namespace A; use function B\\f; f(); F(); g();"),
        s(&[("", "b\\f"), ("", "b\\f"), ("a\\g", "g")])
    );
}

// ----- constants (ns_041..ns_060 family) ---------------------------------------------

/// php: `namespace A; use const B\X; echo X;` → `Undefined constant "B\X"`;
/// `echo x;` → `Undefined constant "A\x"` (constant imports are case-sensitive).
#[test]
fn const_import_is_case_sensitive() {
    assert_eq!(
        consts("<?php namespace A; use const B\\X; echo X, x;"),
        s(&[("", "b\\X"), ("a\\x", "x")])
    );
}

/// php: `namespace A; echo X;` → `Undefined constant "A\X"` (two-step);
/// `\X` → `X`; `namespace\X` → `A\X`; `use B\C; C\X` → `B\C\X`; `B\X` → `A\B\X`.
#[test]
fn constant_name_forms() {
    assert_eq!(
        consts("<?php namespace A; use B\\C; echo X, \\X, namespace\\X, C\\X, B\\X;"),
        s(&[
            ("a\\X", "X"),
            ("", "X"),
            ("", "a\\X"),
            ("", "b\\c\\X"),
            ("", "a\\b\\X")
        ])
    );
}

#[test]
fn constant_keys_lowercase_only_the_namespace_part() {
    assert_eq!(
        consts("<?php namespace Foo\\BAR; echo Baz, \\Qux\\ZED\\Mixed;"),
        s(&[("foo\\bar\\Baz", "Baz"), ("", "qux\\zed\\Mixed")])
    );
}

#[test]
fn global_constant_is_single_step() {
    assert_eq!(
        consts("<?php echo PHP_EOL, E_ALL;"),
        s(&[("", "PHP_EOL"), ("", "E_ALL")])
    );
}

#[test]
fn compiler_halt_offset_folds() {
    let l = lower("<?php echo __COMPILER_HALT_OFFSET__; __halt_compiler(); data");
    let off = l.program().halt_offset.expect("halt offset");
    assert_eq!(l.echoed(), [off.to_string()]);
    // Without `__halt_compiler()` it stays a constant fetch.
    let l = lower("<?php namespace N; echo __COMPILER_HALT_OFFSET__;");
    assert_eq!(l.const_names().len(), 1);
}

// ----- ::class folding ----------------------------------------------------------------

/// php: `namespace N; echo namespace\Foo::class, "|", \Foo::class, "|", Foo\Bar::class, "|", Foo::class;`
/// → `N\Foo|Foo|N\Foo\Bar|N\Foo`; `int::class` → `N\int` (not special here).
#[test]
fn class_constant_folds_on_names() {
    let l = lower("<?php namespace N; echo namespace\\Foo::class, \\Foo::class, Foo\\Bar::class, Foo::class, int::class, Foo::CLASS;");
    assert_eq!(
        l.echoed(),
        ["N\\Foo", "Foo", "N\\Foo\\Bar", "N\\Foo", "N\\int", "N\\Foo"]
    );
}

/// php: `namespace N; class A {} class B extends A { function m(){ return self::class . "|" . parent::class; } } echo (new B)->m();`
/// → `N\B|N\A`; `static::class` stays dynamic.
#[test]
fn self_and_parent_class_fold_in_methods() {
    let l = lower("<?php namespace N; use Other\\Base; class B extends Base { function m(){ echo self::class, parent::class, static::class; } }");
    assert_eq!(
        l.echoed(),
        [
            "N\\B",
            "Other\\Base",
            "<(class-const const=class\n  (static))>"
        ]
    );
}

/// php: in a trait / closure / class constant the scope is not known at
/// compile time, so `self::class` stays a runtime fetch.
#[test]
fn self_class_stays_dynamic_where_scope_is_unknown() {
    let l = lower("<?php namespace N; trait T { function m(){ echo self::class; } } class C { const X = self::class; function m() { echo (function(){ return self::class; })(); } }");
    let printed = l.print();
    assert_eq!(
        printed.matches("(class-const const=class\n").count(),
        3,
        "{printed}"
    );
}

// ----- diagnostics ----------------------------------------------------------------------

/// php: `use Foo\Bar; use Baz\Bar;` → `Cannot use Baz\Bar as Bar because the name is already in use`
#[test]
fn duplicate_class_import() {
    let l = lower("<?php use Foo\\Bar; use Baz\\Bar;");
    assert_eq!(
        l.errors(),
        ["RPHP_E0200 Cannot use Baz\\Bar as Bar because the name is already in use"]
    );
}

/// php: `use function Foo\f; use function Bar\F;` → `Cannot use function Bar\F as F because the name is already in use`
/// php: `use const Foo\C; use const Bar\C;` → `Cannot use const Bar\C as C because the name is already in use`
/// php: `use const Foo\C; use const Bar\c;` → ok
#[test]
fn duplicate_function_and_const_imports() {
    let l = lower("<?php use function Foo\\f; use function Bar\\F; use const Foo\\C; use const Bar\\C; use const Foo\\D; use const Bar\\d;");
    assert_eq!(
        l.errors(),
        [
            "RPHP_E0200 Cannot use function Bar\\F as F because the name is already in use",
            "RPHP_E0200 Cannot use const Bar\\C as C because the name is already in use",
        ]
    );
}

/// php: `class Bar {} use Foo\Bar;` → `Cannot use Foo\Bar as Bar because the name is already in use`
/// php: `namespace N; if (1) { class C{} } use Bar\C;` → same (conditional declarations count)
/// php: `namespace N; use function N\strlen; function strlen(){}` → ok (same name)
#[test]
fn import_after_declaration_conflicts() {
    let l = lower("<?php class Bar {} use Foo\\Bar;");
    assert_eq!(
        l.errors(),
        ["RPHP_E0200 Cannot use Foo\\Bar as Bar because the name is already in use"]
    );
    let l = lower("<?php namespace N; if (1) { class C {} } use Bar\\C;");
    assert_eq!(
        l.errors(),
        ["RPHP_E0200 Cannot use Bar\\C as C because the name is already in use"]
    );
    let l = lower(
        "<?php namespace N; function strlen(){} use function N\\strlen; class Foo {} use N\\Foo;",
    );
    assert!(l.errors().is_empty(), "{:?}", l.errors());
}

/// php: `use Foo\Bar; class Bar {}` → `Cannot redeclare class Bar (previously declared as local import)`
/// php: `namespace N; use function Foo\f; function f(){}` → `Cannot redeclare function N\f() (previously declared as local import)`
/// php: `namespace N; use const Bar\X; const X = 1;` → `Cannot declare const N\X because the name is already in use`
/// php: `namespace A; use A\Foo; class Foo {}` → ok
#[test]
fn declaration_after_import_conflicts() {
    let l = lower("<?php use Foo\\Bar; class Bar {}");
    assert_eq!(
        l.errors(),
        ["RPHP_E0200 Cannot redeclare class Bar (previously declared as local import)"]
    );
    let l = lower("<?php namespace N; use function Foo\\f; function f(){}");
    assert_eq!(
        l.errors(),
        ["RPHP_E0200 Cannot redeclare function N\\f() (previously declared as local import)"]
    );
    let l = lower("<?php namespace N; use const Bar\\X; const X = 1;");
    assert_eq!(
        l.errors(),
        ["RPHP_E0200 Cannot declare const N\\X because the name is already in use"]
    );
    let l = lower("<?php namespace A; use A\\Foo; class Foo {} use function A\\f; function f() {} use const A\\X; const X = 1;");
    assert!(l.errors().is_empty(), "{:?}", l.errors());
}

/// php: `use Foo as int;` → `Cannot use Foo as int because 'int' is a special class name`
#[test]
fn reserved_alias() {
    let l = lower("<?php use Foo as int; use Bar\\Mixed;");
    assert_eq!(
        l.errors(),
        [
            "RPHP_E0201 Cannot use Foo as int because 'int' is a special class name",
            "RPHP_E0201 Cannot use Bar\\Mixed as Mixed because 'Mixed' is a special class name",
        ]
    );
}

/// php: `use Foo;` → `Warning: The use statement with non-compound name 'Foo' has no effect`
/// (no warning inside a namespace or with an alias).
#[test]
fn non_compound_use_warns_at_global_scope() {
    let l = lower("<?php use Foo; use function strlen; use Bar as Baz;");
    assert_eq!(
        l.messages(),
        [
            "The use statement with non-compound name 'Foo' has no effect",
            "The use statement with non-compound name 'strlen' has no effect",
        ]
    );
    assert!(l.diags.iter().all(|d| !d.is_error()));
    let l = lower("<?php namespace N; use Qux;");
    assert!(l.diags.is_empty(), "{:?}", l.messages());
}

/// php: `class Int {}` → `Cannot use "Int" as a class name as it is reserved`;
/// `interface bool {}` → `... as an interface name ...`; `trait float {}`;
/// `enum void {}`.
#[test]
fn reserved_class_names() {
    let l = lower("<?php class Int {} interface bool {} trait float {} enum void {} class Iterable {} class Ok {}");
    assert_eq!(
        l.errors(),
        [
            "RPHP_E0201 Cannot use \"Int\" as a class name as it is reserved",
            "RPHP_E0201 Cannot use \"bool\" as an interface name as it is reserved",
            "RPHP_E0201 Cannot use \"float\" as a trait name as it is reserved",
            "RPHP_E0201 Cannot use \"void\" as an enum name as it is reserved",
            "RPHP_E0201 Cannot use \"Iterable\" as a class name as it is reserved",
        ]
    );
}

/// php: `class A extends self {}` → `Cannot use "self" as class name, as it is reserved`;
/// `class A implements parent {}` → `... as interface name ...`;
/// `class A { use static; }` → `... as trait name ...`;
/// `try {} catch (self $e) {}` → `Bad class name in the catch statement`
/// (the adapter already rejects that one and drops the type);
/// `new \self;` → `'\self' is an invalid class name`.
#[test]
fn scope_keywords_where_names_are_required() {
    let (l, _) = lower_lenient("<?php class A extends self implements parent { use static; }");
    assert_eq!(
        l.errors(),
        [
            "RPHP_E0201 Cannot use \"self\" as class name, as it is reserved",
            "RPHP_E0201 Cannot use \"parent\" as interface name, as it is reserved",
            "RPHP_E0201 Cannot use \"static\" as trait name, as it is reserved",
        ]
    );
    let (l, parse) = lower_lenient("<?php try {} catch (self $e) {} new \\self;");
    assert_eq!(l.errors(), ["RPHP_E0201 '\\self' is an invalid class name"]);
    assert_eq!(parse[0].message, "Bad class name in the catch statement");
}

/// php: `function f(){ return self::X; }` → `Cannot use "self" when no class scope is active`
/// (also `static`, `new static`, `instanceof self`, `self::$p`, `self::m()`);
/// `class A { function m(){ return parent::X; } }` → `Cannot use "parent" when current class scope has no parent`.
/// At top level and in closures php defers to runtime; `function f($x = self::X) {}`
/// is accepted (constant-expression position).
#[test]
fn scope_keyword_checks() {
    let (l, _) = lower_lenient("<?php function f(){ return self::X . static::X . parent::X . (new static) . ($x instanceof self) . self::$p . self::m(); }");
    assert_eq!(
        l.errors(),
        [
            "RPHP_E0204 Cannot use \"self\" when no class scope is active",
            "RPHP_E0204 Cannot use \"static\" when no class scope is active",
            "RPHP_E0204 Cannot use \"parent\" when no class scope is active",
            "RPHP_E0204 Cannot use \"static\" when no class scope is active",
            "RPHP_E0204 Cannot use \"self\" when no class scope is active",
            "RPHP_E0204 Cannot use \"self\" when no class scope is active",
            "RPHP_E0204 Cannot use \"self\" when no class scope is active",
        ]
    );
    let (l, _) =
        lower_lenient("<?php class A { function m(){ return parent::X; } static $s = parent::Y; }");
    assert_eq!(
        l.errors(),
        ["RPHP_E0205 Cannot use \"parent\" when current class scope has no parent"]
    );
    // Unknown scope: no compile-time diagnostic.
    let l = lower("<?php echo self::X; $c = function() { return static::X; }; function f($x = self::X) {} trait T { function m() { return parent::X; } } class B extends A { function m() { return parent::X; } }");
    assert!(l.errors().is_empty(), "{:?}", l.errors());
}

/// php: `namespace A { } namespace D;` → `Cannot mix bracketed namespace declarations with unbracketed namespace declarations`
#[test]
fn namespace_mixing() {
    let (l, _) = lower_lenient("<?php namespace A { } namespace D; echo 1;");
    assert!(
        l.codes().contains(&codes::NAMESPACE_MIX),
        "{:?}",
        l.messages()
    );
    let (l, _) = lower_lenient("<?php namespace A; namespace D { }");
    assert!(
        l.codes().contains(&codes::NAMESPACE_MIX),
        "{:?}",
        l.messages()
    );
    let l = lower("<?php namespace A; echo 1; namespace B; echo 2;");
    assert!(l.diags.is_empty());
}

/// php: `function f(){} function F(){}` → `Cannot redeclare function F() (previously declared in <file>:1)`;
/// `namespace N; class A{} class a{}` → `Cannot redeclare class N\A (previously declared in ...)`;
/// `if (1) { function f(){} } function f(){}` → no compile error (conditional).
#[test]
fn duplicate_top_level_declarations() {
    let l = lower("<?php\nfunction f(){}\nfunction F(){}");
    assert_eq!(
        l.errors(),
        ["RPHP_E0102 Cannot redeclare function F() (previously declared in /tmp/t.php:2)"]
    );
    let l = lower("<?php namespace N; class A{} interface I {} { class a{} } enum i {}");
    assert_eq!(
        l.errors(),
        [
            "RPHP_E0106 Cannot redeclare class N\\A (previously declared in /tmp/t.php:1)",
            "RPHP_E0106 Cannot redeclare interface N\\I (previously declared in /tmp/t.php:1)",
        ]
    );
    let l = lower("<?php if (1) { function f(){} } function f(){}");
    assert!(l.errors().is_empty(), "{:?}", l.errors());
    let l = lower("<?php namespace A { function g(){} } namespace B { function g(){} }");
    assert!(l.errors().is_empty(), "{:?}", l.errors());
}

#[test]
fn use_and_namespace_names_stay_declarations() {
    let l = lower("<?php namespace A\\B; use C\\D as E; new E;");
    // The `use` item and the namespace name carry no resolution; the
    // reference does.
    let printed = l.print();
    assert!(printed.contains("(name A\\B kind=q)\n"), "{printed}");
    assert!(printed.contains("(name C\\D kind=q))"), "{printed}");
    assert_eq!(printed.matches("resolved=").count(), 1, "{printed}");
    assert!(
        printed.contains("(name E kind=u resolved=class fqn=C\\D key=c\\d)"),
        "{printed}"
    );
}
