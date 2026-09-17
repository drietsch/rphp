//! `arrow_free_vars` against php 8.5.0's implicit binding, observed through
//! `ReflectionFunction::getStaticVariables()` with every variable defined in
//! the enclosing scope (php silently skips undefined ones).

mod common;

use rphp_ast::v2::visit::{walk_expr, Visitor};
use rphp_ast::v2::{ArrowFn, Expr};
use rphp_hir::free_vars::arrow_free_vars;

fn captures(src: &str) -> Vec<Vec<String>> {
    let l = common::lower(src);
    struct Find(Vec<ArrowFn>);
    impl Visitor for Find {
        fn visit_expr(&mut self, e: &Expr) {
            if let Expr::ArrowFn(f) = e {
                self.0.push((**f).clone());
                // Only outermost arrow functions are collected.
                return;
            }
            walk_expr(self, e);
        }
    }
    let mut f = Find(Vec::new());
    f.visit_program(l.program());
    f.0.iter()
        .map(|a| {
            arrow_free_vars(a, &l.interner)
                .into_iter()
                .map(|id| l.text(id))
                .collect()
        })
        .collect()
}

/// php: `fn() => $u = 1` captures `u`; `fn() => $arr[$k] = 1` captures
/// `arr,k`; `fn() => $k++` captures `k`; `fn() => $u = &$k` captures `u,k`.
#[test]
fn writes_and_targets_are_captured() {
    assert_eq!(
        captures("<?php fn() => $u = 1; fn() => $arr[$k] = 1; fn() => $k++; fn() => $u = &$k;"),
        [vec!["u"], vec!["arr", "k"], vec!["k"], vec!["u", "k"]]
    );
}

/// php: `fn($p) => $p + $v0 + (fn($v0) => $v0 + $v1)(1)` captures `v0,v1`:
/// only the outer function's parameters are removed.
#[test]
fn nested_arrow_params_are_not_removed() {
    assert_eq!(
        captures(
            "<?php fn($p) => $p + $v0 + (fn($v0) => $v0 + $v1)(1); fn() => fn() => fn() => $a1;"
        ),
        [vec!["v0", "v1"], vec!["a1"]]
    );
}

/// php: `fn() => function() use ($cap) { return $cap . $cc; }` captures
/// `cap` only; `fn() => function() { return $w; }` captures nothing;
/// `fn() => function() { static $u; return $u; }` captures nothing.
#[test]
fn closures_contribute_only_their_use_list() {
    assert_eq!(
        captures("<?php fn() => function() use ($cap) { return $cap . $cc; }; fn() => function() { return $w; }; fn() => function() { static $u; return $u; };"),
        [vec!["cap"], vec![], vec![]]
    );
}

/// php: `fn() => compact("cc") + [$$dyn]` captures `dyn`;
/// `fn() => "$str {$obj->p} $nm"` captures `str,obj,nm`;
/// `fn() => [$a1, $a2] = $src` captures `a1,a2,src` (on the lowered tree
/// the destructuring is already a `Let` over `$src`, so the order is
/// `src,a1,a2` — only `getStaticVariables()` ordering can tell);
/// `fn() => match($u) { $k => $w, default => $w2 }` captures `u,k,w,w2`;
/// `fn() => throw new Exception($str)` captures `str`.
#[test]
fn expression_forms() {
    assert_eq!(
        captures("<?php fn() => compact('cc') + [$$dyn]; fn() => \"$str {$obj->p} $nm\"; fn() => [$a1, $a2] = $src; fn() => match($u) { $k => $w, default => $w2 }; fn() => throw new Exception($str); fn() => isset($u) && empty($k);"),
        [
            vec!["dyn"],
            vec!["str", "obj", "nm"],
            vec!["src", "a1", "a2"],
            vec!["u", "k", "w", "w2"],
            vec!["str"],
            vec!["u", "k"],
        ]
    );
}

/// php: `fn() => $this`, `fn() => [$GLOBALS["g"], $_SERVER["x"], $_GET, $_POST, $_COOKIE, $_FILES, $_ENV, $_REQUEST, $_SESSION]`
/// capture nothing; `static fn() => $k` captures `k`.
#[test]
fn this_and_auto_globals_are_excluded() {
    assert_eq!(
        captures("<?php fn() => $this->x; fn() => [$GLOBALS['g'], $_SERVER['x'], $_GET, $_POST, $_COOKIE, $_FILES, $_ENV, $_REQUEST, $_SESSION]; static fn() => $k; fn() => new class { function m() { return $hidden; } };"),
        [vec![], vec![], vec!["k"], vec![]]
    );
}
