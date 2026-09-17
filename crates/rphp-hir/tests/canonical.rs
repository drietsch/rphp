//! `Hir::validate_canonical`: the raw parser output violates the invariant
//! in every documented way, the lowered tree in none.

mod common;

use rphp_ast::v2::Program;
use rphp_hir::Hir;
use rphp_intern::Interner;
use rphp_parser::{parse_v2, ParseOptions};
use rphp_span::FileId;

fn parse(src: &str) -> (Program, Interner) {
    let mut interner = Interner::new();
    let parsed = parse_v2(src.as_bytes(), ParseOptions::new(FileId(0)), &mut interner);
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    (parsed.program, interner)
}

fn violations(src: &str) -> Vec<String> {
    let (program, _) = parse(src);
    Hir(program)
        .validate_canonical()
        .into_iter()
        .map(|v| v.what)
        .collect()
}

#[test]
fn raw_tree_violates_each_rule() {
    assert_eq!(
        violations("<?php if ($a) {} elseif ($b) {}"),
        ["elseif clause"]
    );
    assert_eq!(violations("<?php $a ??= 1;"), ["??= assignment"]);
    assert_eq!(violations("<?php [$a] = $b;"), ["destructuring assignment"]);
    assert_eq!(violations("<?php $a ?: 1;"), ["short ternary"]);
    assert_eq!(
        violations("<?php $o?->a; $o?->m();"),
        ["nullsafe link", "nullsafe link"]
    );
    assert_eq!(
        violations("<?php $x |> strlen(...);"),
        ["pipe operator", "unresolved function name"]
    );
    assert_eq!(
        violations("<?php isset($a, $b);"),
        ["isset with several operands"]
    );
    assert_eq!(
        violations("<?php list($a) = $b;"),
        ["destructuring assignment", "list() syntax"]
    );
    assert_eq!(
        violations("<?php foreach ($x as [$a]) {}"),
        ["foreach destructuring target"]
    );
    assert_eq!(
        violations("<?php echo FOO, __LINE__, Foo::class;"),
        [
            "unresolved constant",
            "unsubstituted magic constant",
            "unfolded Name::class"
        ]
    );
    assert_eq!(
        violations("<?php new Foo; foo(); function f(Foo $x) {}"),
        [
            "unresolved class name",
            "unresolved function name",
            "unresolved type name"
        ]
    );
    assert_eq!(
        violations("<?php class A { public $p { get => 1; set => 1; } }"),
        [
            "hook expression body",
            "hook expression body",
            "set hook without parameter list"
        ]
    );
    assert_eq!(
        violations(
            "<?php readonly class R { public int $x; function __construct(public int $y) {} }"
        ),
        [
            "property of a readonly class without readonly",
            "promoted parameter of a readonly class without readonly"
        ]
    );
    assert_eq!(
        violations(
            "<?php #[Attr] class A extends B implements C { use T; } try {} catch (E $e) {}"
        ),
        [
            "unresolved attribute name",
            "unresolved class name",
            "unresolved class name",
            "unresolved name",
            "unresolved catch type"
        ]
    );
}

#[test]
fn dynamic_magic_constants_are_allowed() {
    let l = common::lower("<?php trait T { function m() { return __CLASS__; } } $o = new class { function m() { return __METHOD__ . __CLASS__ . (function() { return __FUNCTION__; })(); } };");
    assert!(l.hir.validate_canonical().is_empty());
}

#[test]
fn lowered_trees_are_canonical() {
    let src = "<?php namespace N; use A\\B; if ($a) {} elseif ($b) {} $a ??= $o?->x ?: [$p, [&$q]] = $r |> f(...); foreach ($m as [$k]) { echo isset($a, $b), FOO, __LINE__, B::class, self::class; } class C extends B { public $p { get => 1; set => 2; } function m() { return static::class; } } readonly class R { function __construct(public int $x) {} }";
    let l = common::lower(src);
    assert!(l.diags.is_empty(), "{:?}", l.messages());
    assert!(
        l.hir.validate_canonical().is_empty(),
        "{:?}",
        l.hir.validate_canonical()
    );
}
