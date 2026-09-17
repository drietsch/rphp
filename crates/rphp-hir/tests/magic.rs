//! Magic-constant substitution, compared with php 8.5.0's output. Every
//! expected string below was produced by running the same snippet with
//! `php -n` from a file at `/tmp/t.php` (or `php -n -r` where noted).

mod common;

use std::path::PathBuf;

use common::{lower, lower_with};

/// The printed tree with indentation removed, for order-sensitive checks.
fn flat(printed: &str) -> String {
    printed
        .lines()
        .map(str::trim_start)
        .collect::<Vec<_>>()
        .join("\n")
}

/// php: `echo __NAMESPACE__, "|", __FILE__, "|", __DIR__, "|", __LINE__;`
/// in `/tmp/t.php` under `namespace Foo;` → `Foo|/tmp/t.php|/tmp|<line>`.
/// At top level `__FUNCTION__`, `__METHOD__`, `__CLASS__`, `__TRAIT__` and
/// `__PROPERTY__` are all `""`.
#[test]
fn top_level_values() {
    let l = lower("<?php\nnamespace Foo;\necho __NAMESPACE__, __FILE__, __DIR__, __LINE__,\n__LINE__, __FUNCTION__, __METHOD__, __CLASS__, __TRAIT__, __PROPERTY__;");
    assert_eq!(
        l.echoed(),
        ["Foo", "/tmp/t.php", "/tmp", "3", "4", "", "", "", "", ""]
    );
}

#[test]
fn dir_of_root_file_and_no_file() {
    let l = lower_with(
        "<?php echo __FILE__, __DIR__;",
        Some(PathBuf::from("/a.php")),
        None,
    );
    assert_eq!(l.echoed(), ["/a.php", "/"]);
    let l = lower_with("<?php echo __FILE__, __DIR__;", None, None);
    assert_eq!(l.echoed(), ["", ""]);
}

/// php: `eval('return __FILE__ . "|" . __DIR__ . "|" . __LINE__ . "|" . __FUNCTION__ . "|" . __NAMESPACE__;')`
/// called from line 23 of `/tmp/t.php` inside `namespace Foo` and inside a
/// function → `/tmp/t.php(23) : eval()'d code|/tmp|1|` + `` + `` — eval'd
/// code is its own pseudo-main: no function, no namespace, `__DIR__` of the
/// script.
#[test]
fn eval_code_naming() {
    let l = lower_with(
        "<?php echo __FILE__, __DIR__, __LINE__, __FUNCTION__, __NAMESPACE__, __CLASS__;",
        Some(PathBuf::from("/tmp/t.php")),
        Some("/tmp/t.php(23) : eval()'d code"),
    );
    assert_eq!(
        l.echoed(),
        ["/tmp/t.php(23) : eval()'d code", "/tmp", "1", "", "", ""]
    );
}

/// php: `namespace Foo; function g(){ return __FUNCTION__ . "|" . __METHOD__ . "|" . __CLASS__; }`
/// → `Foo\g|Foo\g|`.
#[test]
fn free_function() {
    let l = lower(
        "<?php namespace Foo; function g(){ echo __FUNCTION__, __METHOD__, __CLASS__, __TRAIT__; }",
    );
    assert_eq!(l.echoed(), ["Foo\\g", "Foo\\g", "", ""]);
}

/// php: `namespace Foo; class D { static function s() { return __FUNCTION__ . "|" . __METHOD__; } }`
/// → `s|Foo\D::s`; `__CLASS__` → `Foo\D`.
#[test]
fn method() {
    let l = lower("<?php namespace Foo; class D { static function s() { echo __FUNCTION__, __METHOD__, __CLASS__, __TRAIT__; } }");
    assert_eq!(l.echoed(), ["s", "Foo\\D::s", "Foo\\D", ""]);
}

/// php (`/tmp/t.php`):
/// ```php
/// namespace Foo;
/// $c = function(){ return __FUNCTION__ . "|" . __METHOD__; };   // line 3
/// function f(){
///     $c = function(){                                              // line 5
///         $d = function(){ return __FUNCTION__ . "|" . __METHOD__; }; // line 6
///         return $d();
///     };
///     return $c();
/// }
/// class B { static function s(){ return (fn() => __FUNCTION__ . "|" . __METHOD__ . "|" . __CLASS__)(); } } // line 11
/// ```
/// prints `{closure:/tmp/t.php:3}|{closure:/tmp/t.php:3}`,
/// `{closure:{closure:Foo\f():5}:6}|{closure:{closure:Foo\f():5}:6}` and
/// `{closure:Foo\B::s():11}|{closure:Foo\B::s():11}|Foo\B`.
#[test]
fn closure_names_php84() {
    let src = "<?php
namespace Foo;
$c = function(){ echo __FUNCTION__, __METHOD__, __CLASS__; };
function f(){
    $c = function(){
        $d = function(){ echo __FUNCTION__, __METHOD__; };
        return $d();
    };
    return $c();
}
class B { static function s(){ return (fn() => print(__FUNCTION__ . __METHOD__ . __CLASS__))(); } }
";
    let l = lower(src);
    let printed = l.print();
    for expected in [
        "(str \"{closure:/tmp/t.php:3}\")",
        "(str \"{closure:{closure:Foo\\\\f():5}:6}\")",
        "(str \"{closure:Foo\\\\B::s():11}\")",
    ] {
        assert!(
            printed.contains(expected),
            "missing {expected} in\n{printed}"
        );
    }
    // Top-level closure: `__CLASS__` is "".
    assert!(
        flat(&printed).contains(
            "(str \"{closure:/tmp/t.php:3}\")\n(str \"{closure:/tmp/t.php:3}\")\n(str \"\")"
        ),
        "{printed}"
    );
    // The arrow function in a method sees the class.
    assert!(printed.contains("(str \"Foo\\\\B\")"), "{printed}");
}

/// php: closures in eval'd code are named after the eval:
/// `eval('return (function(){ return __FUNCTION__; })();')` at line 4 of
/// `/tmp/t.php` → `{closure:/tmp/t.php(4) : eval()'d code:1}`.
#[test]
fn closure_in_eval() {
    let l = lower_with(
        "<?php echo (function(){ return __FUNCTION__; })();",
        Some(PathBuf::from("/tmp/t.php")),
        Some("/tmp/t.php(4) : eval()'d code"),
    );
    assert!(
        l.print()
            .contains("(str \"{closure:/tmp/t.php(4) : eval()'d code:1}\")"),
        "{}",
        l.print()
    );
}

/// php: `-r` code: `__FILE__` is `Command line code`, closures
/// `{closure:Command line code:1}`, `__DIR__` the working directory.
#[test]
fn command_line_code() {
    let mut sources = rphp_source::SourceMap::new();
    let fid = sources.add(
        "-",
        "<?php echo __FILE__, __DIR__, (function(){ return __FUNCTION__; })();".as_bytes(),
    );
    let mut interner = rphp_intern::Interner::new();
    let parsed = rphp_parser::parse_v2(
        "<?php echo __FILE__, __DIR__, (function(){ return __FUNCTION__; })();".as_bytes(),
        rphp_parser::ParseOptions::new(fid),
        &mut interner,
    );
    let line_of = |s: rphp_span::Span| sources.get(s.file).line_col(s.lo).0;
    let opts = rphp_hir::LowerOptions::new(&line_of)
        .with_eval_name("Command line code")
        .with_dir("/work");
    let (hir, diags) = rphp_hir::lower(parsed.program, &mut interner, &opts);
    assert!(diags.is_empty());
    let printed = rphp_hir::print(&hir, &interner);
    assert!(printed.contains("(str \"Command line code\")"), "{printed}");
    assert!(printed.contains("(str \"/work\")"), "{printed}");
    assert!(
        printed.contains("(str \"{closure:Command line code:1}\")"),
        "{printed}"
    );
}

/// php: `namespace Foo; trait T { function t(){ return __CLASS__ . "|" . __TRAIT__ . "|" . __METHOD__ . "|" . __FUNCTION__; } } class C { use T; } echo (new C)->t();`
/// → `Foo\C|Foo\T|Foo\T::t|t`: `__CLASS__` is the *using* class, so it stays
/// a `MagicConst` for the compiler; the rest is static.
#[test]
fn trait_keeps_class_dynamic() {
    let l = lower("<?php namespace Foo; trait T { function t(){ echo __CLASS__, __TRAIT__, __METHOD__, __FUNCTION__, (function(){ return __CLASS__; })(); } }");
    assert_eq!(l.echoed()[..4], ["<__CLASS__>", "Foo\\T", "Foo\\T::t", "t"]);
    // Inside the nested closure too.
    assert!(
        flat(&l.print()).contains("(return\n(magic __CLASS__))"),
        "{}",
        l.print()
    );
}

/// php: `namespace Foo; class D { public $p { get { return __PROPERTY__ . "|" . __FUNCTION__ . "|" . __METHOD__; } } } echo (new D)->p;`
/// → `p|$p::get|Foo\D::$p::get`; a closure inside a hook is named
/// `{closure:Foo\D::$p::get():<line>}` and sees `__PROPERTY__` as `""`.
#[test]
fn property_hooks() {
    let l = lower("<?php namespace Foo; class D { public $p { get { echo __PROPERTY__, __FUNCTION__, __METHOD__, (function(){ return __PROPERTY__ . __FUNCTION__; })(); return 1; } set { echo __PROPERTY__, __FUNCTION__; } } }");
    let printed = l.print();
    for expected in [
        "(str \"p\")",
        "(str \"$p::get\")",
        "(str \"Foo\\\\D::$p::get\")",
        "(str \"\")",
        "(str \"{closure:Foo\\\\D::$p::get():1}\")",
        "(str \"$p::set\")",
    ] {
        assert!(
            printed.contains(expected),
            "missing {expected} in\n{printed}"
        );
    }
}

/// php: `namespace Foo; class E2 { const C = __CLASS__ . "|" . __FUNCTION__ . "|" . __METHOD__; public $d = __CLASS__; function __construct(public $q = __CLASS__ . __FUNCTION__ . __METHOD__) {} }`
/// → `Foo\E2||` for the constant, `Foo\E2` for the default, and
/// `Foo\E2__constructFoo\E2::__construct` for the parameter default.
#[test]
fn class_body_positions() {
    let l = lower("<?php namespace Foo; class E2 { const C = [__CLASS__, __FUNCTION__, __METHOD__]; public $d = __CLASS__; function __construct(public $q = [__CLASS__, __FUNCTION__, __METHOD__]) {} }");
    let printed = l.print();
    assert!(flat(&printed).contains("(item C\n(array syntax=short\n(item\n(str \"Foo\\\\E2\"))\n(item\n(str \"\"))\n(item\n(str \"\"))"), "{printed}");
    assert!(
        flat(&printed).contains("(item $d\n(str \"Foo\\\\E2\"))"),
        "{printed}"
    );
    assert!(printed.contains("(str \"__construct\")"), "{printed}");
    assert!(
        printed.contains("(str \"Foo\\\\E2::__construct\")"),
        "{printed}"
    );
}

/// php: `function f(){ class A { const X = __FUNCTION__ . "|" . __METHOD__ . "|" . __CLASS__; } return A::X; } echo f();`
/// → `f||A`: a class nested in a function sees the function for
/// `__FUNCTION__` but `__METHOD__` is `""` directly in a class body.
#[test]
fn class_nested_in_function() {
    let l =
        lower("<?php function f(){ class A { const X = [__FUNCTION__, __METHOD__, __CLASS__]; } }");
    let printed = l.print();
    assert!(printed.contains("(str \"f\")"), "{printed}");
    assert!(printed.contains("(str \"\")"), "{printed}");
    assert!(printed.contains("(str \"A\")"), "{printed}");
}

/// php: `class D { function m() { function nested() { return __CLASS__ . "|" . __FUNCTION__ . "|" . __METHOD__ . "|" . (function(){ return __FUNCTION__; })(); } return nested(); } } echo (new D)->m();`
/// → `|nested|nested|{closure:nested():1}`: a free function declared inside
/// a method does not see the class.
#[test]
fn free_function_inside_method_clears_class() {
    let l = lower("<?php class D { function m() { function nested() { echo __CLASS__, __FUNCTION__, __METHOD__, (function(){ return __FUNCTION__; })(); } } }");
    let printed = l.print();
    assert!(
        flat(&printed).contains("(echo\n(str \"\")\n(str \"nested\")\n(str \"nested\")"),
        "{printed}"
    );
    assert!(
        printed.contains("(str \"{closure:nested():1}\")"),
        "{printed}"
    );
}

/// php: inside `new class { ... }` the class name is compiler-generated
/// (`class@anonymous<NUL>/tmp/t.php:1$0`), so `__CLASS__`, `__METHOD__` and
/// the names of closures declared inside stay dynamic; `__FUNCTION__` of a
/// method is still static.
#[test]
fn anonymous_class_stays_dynamic() {
    let l = lower("<?php $o = new class { const X = __CLASS__; function m(){ echo __CLASS__, __METHOD__, __FUNCTION__, (function(){ return __FUNCTION__ . __METHOD__; })(); } };");
    let printed = l.print();
    assert_eq!(printed.matches("(magic __CLASS__)").count(), 2, "{printed}");
    assert_eq!(
        printed.matches("(magic __METHOD__)").count(),
        2,
        "{printed}"
    );
    assert_eq!(
        printed.matches("(magic __FUNCTION__)").count(),
        1,
        "{printed}"
    );
    assert!(printed.contains("(str \"m\")"), "{printed}");
}

/// php: `namespace Foo; enum En { case A; function m() { return __CLASS__ . "|" . __METHOD__; } }` → `Foo\En|Foo\En::m`;
/// `interface I { const X = __CLASS__; }` → `Foo\I`.
#[test]
fn enums_and_interfaces() {
    let l = lower("<?php namespace Foo; enum En { case A; function m() { echo __CLASS__, __METHOD__; } } interface I { const X = __CLASS__; }");
    assert_eq!(l.echoed(), ["Foo\\En", "Foo\\En::m"]);
    assert!(
        flat(&l.print()).contains("(item X\n(str \"Foo\\\\I\"))"),
        "{}",
        l.print()
    );
}

/// php: `__LINE__` is the line of the token itself, not of the statement
/// (`echo\n__LINE__;` prints the second line).
#[test]
fn line_is_the_tokens_line() {
    let l = lower("<?php\n\necho __LINE__,\n__LINE__; function f() { return\n__LINE__; }");
    assert_eq!(l.echoed(), ["3", "4"]);
    assert!(
        flat(&l.print()).contains("(return\n(int 5))"),
        "{}",
        l.print()
    );
}
