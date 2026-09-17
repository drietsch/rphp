<?php
// Lvalue chains and write contexts: nested element writes with
// autovivification, appends at every level, property/element mixes,
// destructuring (positional, keyed, nested, skipped slots, by reference,
// in foreach), unset() of variables / elements / properties, and the
// isset() / empty() quiet chains.

// nested writes autovivify null / missing keys; scalars refuse
$a = [];
$a['x']['y']['z'] = 1;
$a['x']['list'][] = 'p';
$a['x']['list'][] = 'q';
$a[] = 'top';
$a[5][] = 'five';
$a[][] = 'anon';
echo json_encode($a), "\n";
$m = null;
$m[2][3] = 'v';
$m[2][] = 'w';
echo json_encode($m), "\n";

// objects inside arrays and arrays inside objects
$o = new stdClass;
$o->list[] = 1;
$o->list['k'][] = 2;
$o->child = new stdClass;
$o->child->deep['x'] = 'y';
$o->child->deep['x'] .= 'z';
$tbl = ['row' => $o];
$tbl['row']->child->n = 5;
$tbl['row']->list[] = 3;
var_dump($o);
$objs = [new stdClass, new stdClass];
$objs[1]->name = 'second';
$objs[0]->tags[] = 't';
echo json_encode($objs), "\n";

// compound assignment and inc/dec down a chain
$stats = ['hits' => ['a' => 1]];
$stats['hits']['a'] += 2;
$stats['hits']['b'] ??= 10;
$stats['hits']['a']++;
$stats['hits']['c'] = ($stats['hits']['c'] ?? 0) + 1;
$stats['label'] = 'x';
$stats['label'] .= 'y';
echo json_encode($stats), "\n";

// destructuring
[$p, $q] = [1, 2];
[$p, $q] = [$q, $p];
list($r, , $s) = [10, 20, 30];
['a' => $ka, 'b' => $kb] = ['b' => 'B', 'a' => 'A'];
[[$n1, $n2], [$n3]] = [[1, 2], [3]];
[$t1, [$t2, ['deep' => $t3]]] = ['x', ['y', ['deep' => 'z']]];
echo "$p$q $r$s $ka$kb $n1$n2$n3 $t1$t2$t3\n";
$pairs = [[1, 'one'], [2, 'two']];
foreach ($pairs as [$num, $word]) echo "$num=$word ";
foreach ($pairs as $i => [, $w]) echo "$i:$w ";
echo "\n";
$src = [1, 2];
[$x1, &$x2] = $src;
$x2 = 'ref';
echo json_encode($src), "\n";
[$src[1], $src[0]] = $src;          // reads from a snapshot of the source
echo json_encode($src), "\n";
$nested = ['k' => [1, 2]];
[$nested['k'][1], $nested['k'][0]] = $nested['k'];
echo json_encode($nested), "\n";
[$miss1, $miss2] = [1];             // undefined key 1 warns, $miss2 is null
var_dump($miss2);
[$s1] = 'str';                      // list() on a string yields null
var_dump($s1);

// unset
$u = ['a' => 1, 'b' => ['c' => 2, 'd' => 3]];
unset($u['a'], $u['b']['c'], $u['nope']['deeper'], $u['b']['zz']);
echo json_encode($u), "\n";
$uo = new stdClass; $uo->x = 1; $uo->y = 2;
unset($uo->x, $uo->nope);
var_dump($uo);
$uv = 'set';
unset($uv);
var_dump(isset($uv));
$uv = 'again';
echo $uv, "\n";
$idx = [0 => 'a', 1 => 'b', 2 => 'c'];
unset($idx[1]);
$idx[] = 'd';
echo json_encode($idx), "\n";

// isset / empty chains never warn
$chain = ['a' => ['b' => null, 'c' => 0, 'd' => 'x']];
var_dump(
    isset($chain['a']['b']), isset($chain['a']['c']), isset($chain['a']['d']['e']),
    isset($chain['z']['y']['x']), isset($chain['a'], $chain['a']['d']), isset($chain['a'], $chain['a']['b']),
    empty($chain['a']['b']), empty($chain['a']['c']), empty($chain['a']['d']), empty($chain['nope']['x']),
    isset($undefined_var), empty($undefined_var), isset($uo->y), isset($uo->x), empty($uo->y), isset($uo->y->z)
);
$str = 'abc';
var_dump(isset($str[1]), isset($str[5]), isset($str[-1]), empty($str[0]), isset($str['x']));
$nul = null;
var_dump(isset($nul[0]), empty($nul[0]), $nul[0] ?? 'dflt');
