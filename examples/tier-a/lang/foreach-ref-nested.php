<?php
// A by-reference foreach over a nested element (`$this->prop[$key]`,
// `$arr[$k]`, `$obj->list[0]['x']`, a static property's element) iterates
// the container in place: writes through the loop variable land there.
class D {
    private array $listeners = [];
    public static array $table = [];
    function add($e, $p, $l) { $this->listeners[$e][$p][] = $l; }
    function resolve($e) {
        krsort($this->listeners[$e]);
        foreach ($this->listeners[$e] as &$listeners) {
            foreach ($listeners as &$listener) {
                if ($listener[0] instanceof Closure) { $listener[0] = $listener[0](); }
            }
        }
        unset($listeners, $listener);
    }
    function remove($e, $target) {
        foreach ($this->listeners[$e] as $priority => &$listeners) {
            foreach ($listeners as $k => &$v) {
                if ($v === $target) { unset($listeners[$k]); }
            }
            if (!$listeners) { unset($this->listeners[$e][$priority]); }
        }
    }
    function dump() { echo json_encode(array_map(fn($ls) => array_map(fn($l) => is_object($l[0]) ? get_class($l[0]) . ':' . $l[1] : 'x', $ls), $this->listeners['ev'] ?? [])), "\n"; }
}
class S {}
$s = new S;
$d = new D;
$d->add('ev', 255, [fn() => $s, 'a']); $d->add('ev', 0, [fn() => $s, 'b']); $d->add('ev', 0, [fn() => $s, 'c']);
$d->dump();
$d->resolve('ev');
$d->dump();
$d->remove('ev', [$s, 'b']);
$d->dump();
$d->remove('ev', [$s, 'a']);
$d->dump();

// Static property element, nested plain arrays, and an object's list.
D::$table['x'] = [1, 2, 3];
foreach (D::$table['x'] as &$n) { $n *= 10; }
unset($n);
echo json_encode(D::$table), "\n";
$arr = ['k' => ['a' => [1, 2], 'b' => [3]]];
foreach ($arr['k'] as $name => &$inner) { foreach ($inner as &$v) { $v = "$name$v"; } }
unset($inner, $v);
echo json_encode($arr), "\n";
$o = new stdClass; $o->list = [['x' => 1], ['x' => 2]];
foreach ($o->list[1] as &$x) { $x = 20; }
unset($x);
foreach ($o->list as &$row) { $row['y'] = $row['x'] + 1; }
unset($row);
echo json_encode($o->list), "\n";
// The loop variable stays bound to the last element after the loop.
$a = [1, 2, 3];
foreach ($a as &$r) {}
$r = 99;
echo json_encode($a), "\n";
// An element that does not exist yet is created (null) by the by-ref fetch.
$m = [];
foreach ($m['new'] ?? [] as &$z) {}
var_dump($m);
$m2 = ['k' => null];
foreach ($m2['k'] as &$z) {}
var_dump($m2);
// After the loop, the container is not itself a reference (only the last
// element, bound to the loop variable).
$m3 = ['k' => [1, 2, 3]];
foreach ($m3['k'] as &$e) { $e++; }
var_dump($m3);
unset($e);
var_dump($m3);
