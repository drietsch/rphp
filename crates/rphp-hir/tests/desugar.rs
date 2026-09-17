//! Desugaring snapshots (`tests/snapshots/desugar__*.snap`), one per rule of
//! the table in `rphp_hir::desugar`, plus the evaluation-order cases. Each
//! evaluation-order case quotes the `php -n -r` one-liner (php 8.5.0) whose
//! printed trace fixed the order the HIR must reproduce.
//!
//! Review with `cargo insta review` (or `INSTA_UPDATE=always cargo test -p
//! rphp-hir --test desugar` to accept).

mod common;

use common::lower;

fn snap(name: &str, src: &str) {
    let l = lower(src);
    assert!(l.diags.is_empty(), "{name}: {:?}", l.messages());
    insta::assert_snapshot!(name, l.print());
}

#[test]
fn elseif_nests() {
    snap(
        "elseif",
        "<?php if ($a) { echo 1; } elseif ($b) { echo 2; } elseif ($c) { echo 3; } else { echo 4; }
if ($a): echo 1; elseif ($b): echo 2; endif;",
    );
}

/// php: `function t($x){echo "[$x]";return $x;} $r = t(0) ?: t(3); $r = t(4) ?: t(5);`
/// → `[0][3]` then `[4]`: the condition is evaluated once and reused.
#[test]
fn short_ternary() {
    snap(
        "short_ternary",
        "<?php $r = t(0) ?: t(3); $q = $a ?: $b ?: $c;",
    );
}

/// php: `$a = ['x' => null]; $a[k('x')] ??= t(1);` → `{x}[1]`;
/// `$a[k('y')] ??= t(2)` with `$a['y'] = 5` → `{y}` only: the key runs
/// once, the default only when needed.
/// php: `$a = []; $i = 0; $a[$i] ??= ($i = 5); var_dump($a);` → `[5 => 5]`:
/// a plain variable index is re-read at write time, so it is *not* bound.
#[test]
fn coalesce_assign_stabilizes() {
    snap(
        "coalesce_assign",
        "<?php $a[k('x')] ??= t(1); $a[$i] ??= ($i = 5); $o->arr[f()][$j] ??= 2; $o->{$n . ''} ??= 1; $$name ??= 3; f()->p ??= 4; $x ??= 5; C::$$dyn ??= 6; $k = 0; $arr[$k++] ??= 5;",
    );
}

/// php: `$o = null; $r = $o?->a->b(t(10))->c; var_dump($r);` → `NULL`
/// without calling `t`; with `$o` an object: `$o?->a?->m(t(11))?->a` prints
/// `[11](m11)` — every link after the null check runs in order.
#[test]
fn nullsafe_chains() {
    snap(
        "nullsafe",
        "<?php $r = $o?->a->b(t(10))->c; $r = $o?->a?->m(t(11))?->a; $r = $o?->a[k('i')]; $r = $o?->a::$s; $r = $o?->a::m(1); $r = ($o?->f)(); $r = $o?->a . $o?->b;",
    );
}

/// php: `isset($o?->a)` is `false` and `empty($o?->a)` is `true` for a null
/// `$o`; `$o?->a ?? 5` is `5`; the plain fetches keep their quiet mode.
#[test]
fn nullsafe_in_isset_empty_coalesce() {
    snap(
        "nullsafe_isset",
        "<?php var_dump(isset($o?->a, $b), empty($o?->a->b), $o?->a ?? 5, $o?->a?->b ?? $o?->c ?? 7, $o->a ?? 1);",
    );
}

/// php: `isset($s, $undefined)` is `isset($s) && isset($undefined)`.
#[test]
fn isset_splits() {
    snap(
        "isset_split",
        "<?php var_dump(isset($a, $b[0], $c->d), isset($x));",
    );
}

/// php: `[$p, $q] = [t(6), t(7)];` → `[6][7]` (source first);
/// `[k('a') => $p, k('b') => $q] = ['b' => t(8), 'a' => t(9)];` → `[8][9]{a}{b}`
/// (keys evaluated in item order, after the source);
/// `$arr = [1, [2, 3]]; [$p, [, $q]] = $arr;` → `1 3` (skipped slot counts).
#[test]
fn destructuring_assignment() {
    snap(
        "destructuring",
        "<?php [$p, $q] = [t(6), t(7)]; [k('a') => $p, k('b') => $q] = f(); [$p, [, $q]] = $arr; list($a, list($b)) = g(); $r = [$h] = [5]; ['x' => ['y' => $deep]] = $src;",
    );
}

/// php: `$arr2 = [1, 2]; [$p, &$q] = $arr2; $q = 9; echo $arr2[1];` → `9`:
/// the reference aliases the *original* element, so the source is
/// stabilized (not copied) and read per item.
#[test]
fn destructuring_by_ref() {
    snap(
        "destructuring_ref",
        "<?php [$p, &$q] = $arr; [&$a, [$b, &$c]] = $m[f()]; [$x, [&$y]] = $o->p;",
    );
}

/// php: `foreach ([[1, 2], [3, 4]] as [$p, $q]) echo "$p$q,";` → `12,34,`;
/// `foreach (['k' => [1, 2]] as $key => ['0' => $p, '1' => $q])` → `k12,`;
/// `foreach ($rows as [&$first])` fetches each row by reference.
#[test]
fn foreach_destructuring() {
    snap(
        "foreach_destructuring",
        "<?php foreach ($rows as [$p, [$q]]) { echo $p; } foreach ($m as $key => ['0' => $p, '1' => $q]) {} foreach ($rows as [&$first, $second]) {}",
    );
}

/// php: `function t($x){echo "[$x]";return $x;} function g(){echo "(g)";return "strval";} $r = t(1) |> g();`
/// → `[1](g)`: the left operand first, then the callee;
/// `t(3) |> (new K)->m(...)` → `[3](new)(m3)`;
/// `t(4) |> strval(...) |> t(...)` → `[4][4]` (left associative).
#[test]
fn pipe() {
    snap(
        "pipe",
        "<?php $r = t(1) |> strval(...) |> $fn |> (fn($v) => $v) |> (new K)->m(...) |> K::s(...) |> 'strval' |> g();",
    );
}

/// php 8.4: `set => expr;` is `set { $this->prop = expr; }`; a `set` hook
/// without a parameter list has an implicit `$value` typed like the
/// property.
#[test]
fn hooks() {
    snap(
        "hooks",
        "<?php abstract class H {
  public int $p { set(int $v) => $v * 2; get => $this->p + 1; }
  public string $q { set => strtolower($value); &get { return $this->q; } }
  abstract public ?array $r { get; set; }
  public function __construct(public string $s = '' { set => trim($value); }) {}
}
interface I { public string $t { get; set; } }",
    );
}

/// php: `readonly class R { public function __construct(public int $x) {} public int $z; }`
/// → every property is readonly (`ReflectionProperty::isReadOnly()`).
#[test]
fn readonly_class_propagates() {
    snap(
        "readonly_class",
        "<?php readonly class R { public function __construct(public int $x, private(set) string $y) {} public int $z; }
final class Plain { public function __construct(public int $x) {} public int $z; }",
    );
}

/// Arrow functions stay nodes (see `rphp_hir::free_vars`); their bodies are
/// desugared and numbered as their own function body.
#[test]
fn arrow_fn_stays_but_body_desugars() {
    snap(
        "arrow_fn",
        "<?php $f = fn($x) => $x ?: ($y ?? 0); $g = static fn() => fn() => $z?->w; $h = function() use ($a) { return $a ?: 1; };",
    );
}

/// Temporaries restart at `t0` in every function body.
#[test]
fn temps_are_per_body() {
    snap(
        "temps_per_body",
        "<?php $a = $b ?: 1; function f() { $c = $d ?: 2; $e = fn() => $g ?: 3; } class K { function m() { [$x] = $y; } public $p { get => $this->q ?: 0; } }",
    );
}

/// Nothing to desugar: the canonical constructs pass through untouched
/// (`for`, `switch`, `match`, `??`, `op=`, `++`, casts, `@`, `(void)`,
/// interpolation, first-class callables, `clone`, `print`, `include`,
/// `yield`, `global`, `static`, `unset`, `goto`).
#[test]
fn canonical_nodes_pass_through() {
    snap(
        "passthrough",
        "<?php for ($i = 0; $i < 3; $i++) { switch ($i) { case 1: break; default: continue 2; } }
$m = match(true) { $a, $b => 1, default => 2 }; $x = $y ?? $z; $x += 1; $x .= 'a'; $c = (int) @$d; (void) f();
echo \"a $b {$c->d} e\"; $f = strlen(...); $g = clone $o; print 1; include 'x.php'; function gen() { yield 1; yield from g(); global $q; static $s = 1; unset($q); a: goto a; }",
    );
}
