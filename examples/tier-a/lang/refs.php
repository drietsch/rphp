<?php
// References: `$a = &$b`, by-reference parameters (variables, elements,
// properties), `foreach` by reference (with the `unset($v)` idiom and
// without it), `use (&$x)` closures, `global`, `static`, `$GLOBALS`
// read / write-through, references inside arrays that survive copies,
// unset() of a reference, and reference semantics through nested writes.

// plain reference binding
$a = 1; $b = &$a; $b = 2;
echo $a, ' ', $b, "\n";                       // 2 2
$b = &$c; $b = 3;                             // rebinding $b leaves $a alone
echo $a, ' ', $b, ' ', $c, "\n";              // 2 3 3
unset($b); $b = 4;
echo $a, ' ', $b, ' ', $c, "\n";              // 2 4 3

// by-reference parameters
function inc(&$x) { $x++; }
function push_ref(&$arr, $v) { $arr[] = $v; }
$n = 1; inc($n); inc($n);
$list = []; push_ref($list, 'a'); push_ref($list, 'b');
echo $n, ' ', implode(',', $list), "\n";      // 3 a,b
$data = ['k' => 5];
inc($data['k']); inc($data['new']);           // element refs, autovivified
$obj = new stdClass; $obj->p = 10;
inc($obj->p); inc($obj->q);                   // property refs, created on demand
var_dump($data, $obj);
function swap(&$x, &$y) { [$x, $y] = [$y, $x]; }
$p = 'p'; $q = 'q'; swap($p, $q);
echo $p, $q, "\n";                            // qp

// foreach by reference: mutates in place; the leftover reference is a classic trap
$nums = [1, 2, 3];
foreach ($nums as &$v) { $v *= 10; }
unset($v);
print_r($nums);
$trap = [1, 2, 3];
foreach ($trap as &$v) {}
foreach ($trap as $v) {}                      // the last element follows the loop
print_r($trap);
unset($v);
$matrix = [[1, 2], [3, 4]];
foreach ($matrix as &$row) { foreach ($row as &$cell) { $cell += 100; } unset($cell); }
unset($row);
echo json_encode($matrix), "\n";
$grow = [1, 2];
foreach ($grow as $k => &$g) { if ($k == 0) { $grow[] = 3; } $g = "v$g"; }
unset($g);
print_r($grow);

// closures capturing by reference vs by value
$count = 0;
$incr = function () use (&$count) { $count++; };
$snap = function () use ($count) { return $count; };
$incr(); $incr();
echo $count, ' ', $snap(), "\n";              // 2 0
$fibs = [];
$fib = function ($n) use (&$fib) { return $n < 2 ? $n : $fib($n - 1) + $fib($n - 2); };
echo $fib(15), "\n";                          // 610

// global and static
$gv = 'g';
function read_global() { global $gv; return $gv; }
function write_global() { global $gv, $created; $gv .= '!'; $created = 'made'; }
write_global();
echo read_global(), ' ', $created, "\n";      // g! made
function counter() { static $n = 0; static $log = []; $n++; $log[] = $n; return implode('', $log); }
counter(); counter();
echo counter(), "\n";                         // 123
function with_default() { static $x = PHP_INT_SIZE * 2; return $x++; }
echo with_default(), with_default(), "\n";    // 1617

// $GLOBALS: snapshot reads, write-through and nested write-through
$GLOBALS['made_via_globals'] = 'yes';
echo $made_via_globals, ' ', $GLOBALS['gv'], ' ', isset($GLOBALS['nope']) ? 'set' : 'unset', "\n";
function via_globals() { $GLOBALS['cnt'] = ($GLOBALS['cnt'] ?? 0) + 1; $GLOBALS['tree']['leaf'][] = $GLOBALS['cnt']; }
via_globals(); via_globals();
echo $cnt, ' ', json_encode($tree), "\n";
$GLOBALS['gv'] = 'over';
echo $gv, "\n";

// references inside arrays survive copies of the array
$shared = 1;
$holder = ['ref' => &$shared, 'val' => 1];
$copy = $holder;
$copy['ref'] = 99; $copy['val'] = 99;
echo $shared, ' ', $holder['ref'], ' ', $holder['val'], "\n";   // 99 99 1
$arr1 = [1, 2, 3];
$arr2 = $arr1;
$r = &$arr2[0]; $r = 'x';
echo implode(',', $arr1), ' ', implode(',', $arr2), "\n";       // 1,2,3 x,2,3

// nested reference targets
$cfg = ['db' => ['host' => 'a']];
$host = &$cfg['db']['host'];
$host = 'b';
$cfg['db']['port'] = &$port;
$port = 5432;
echo json_encode($cfg), "\n";
$o2 = new stdClass;
$o2->items = [];
$ref = &$o2->items;
$ref[] = 'first';
$o2->items[] = 'second';
echo json_encode($o2->items), "\n";

// reference to a reference collapses; identity checks
$x = 1; $y = &$x; $z = &$y; $z = 7;
var_dump($x === $y, $x, $z);

// A by-reference element of an array literal binds the cell, whatever kind
// of place it names — the pattern a deep-clone or a container builder uses
// to keep a handle on what it is walking.
$v = 1;
$arr = ['k' => 10, 2 => 20];
$obj = new stdClass();
$obj->p = 100;
class RefHolder
{
    public static $sp = 'x';
}
$pool = [&$v, &$arr['k'], &$arr[2], &$obj->p, &RefHolder::$sp];
$pool[0] = 'V';
$pool[1] = 'K';
$pool[2] = 'T';
$pool[3] = 'P';
$pool[4] = 'S';
var_dump($v, $arr, $obj->p, RefHolder::$sp);

$name = 'dyn';
$$name = 5;
$indirect = [&$$name];
$indirect[0] = 'D';
var_dump($dyn);

$glob = 7;
$globals = [&$GLOBALS['glob']];
$globals[0] = 'G';
var_dump($glob);

$x = 1;
$y = 2;
$mixed = ['a' => &$x, 'b' => [&$y]];
$mixed['a'] = 'A';
$mixed['b'][0] = 'B';
var_dump($x, $y);

$values = [1, 2, 3];
$refs = [];
foreach ($values as $k => $value) {
    $refs[] = [&$values[$k], $value];
}
$refs[1][0] = 'two';
var_dump($values);

// `&$a[]`: php appends a fresh null element and binds the reference to it —
// the shape an event dispatcher uses to build a list of lazy closures.
$appended = [1, 2];
$slot = &$appended[];
var_dump($appended);
$slot = 'appended';
var_dump($appended);
$holder = new stdClass();
$holder->list = ['k' => 1];
$viaProp = &$holder->list[];
$viaProp = 'via prop';
var_dump($holder->list);
$nested = [];
$deep = &$nested['deep'][];
$deep = 'deep';
var_dump($nested);
$lazy = [];
foreach ([['a'], ['b']] as $listeners) {
    $closure = &$lazy[];
    $closure = static function () use ($listeners) { return $listeners; };
}
var_dump(count($lazy), ($lazy[0])(), ($lazy[1])());
