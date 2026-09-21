<?php
// foreach over the SPL iterators drives them natively (php's get_iterator
// for internal classes): a user subclass overriding any of the five
// methods is stepped through its methods again, hooks and all.
$a = ['x' => 1, 'y' => 2, 'z' => 3];
foreach (new ArrayIterator($a) as $k => $v) echo "$k=$v "; echo "\n";
foreach (new ArrayObject($a) as $k => $v) echo "$k=$v "; echo "\n";
class Loud extends ArrayIterator { function current(): mixed { echo "[c]"; return parent::current() * 10; } }
foreach (new Loud($a) as $k => $v) echo "$k=$v "; echo "\n";
class Counting extends ArrayIterator { public $n = 0; function next(): void { $this->n++; parent::next(); } }
$c = new Counting($a); foreach ($c as $v) {} var_dump($c->n);
$q = new SplQueue; $q->enqueue('a'); $q->enqueue('b'); foreach ($q as $k => $v) echo "$k=$v "; foreach ($q as $v) echo $v; echo " ", count($q), "\n";
$st = new SplStack; $st->push(1); $st->push(2); foreach ($st as $k => $v) echo "$k=$v "; echo "\n";
$s = new SplObjectStorage; $o1 = new stdClass; $o2 = new stdClass; $s[$o1] = 'i1'; $s[$o2] = 'i2'; foreach ($s as $i => $o) { echo $i, ':', $s[$o], ' '; } echo "\n";
$h = new SplMinHeap; foreach ([3, 1, 2] as $v) $h->insert($v); foreach ($h as $k => $v) echo "$k=$v "; echo count($h), "\n";
$pq = new SplPriorityQueue; $pq->insert('lo', 1); $pq->insert('hi', 9); foreach ($pq as $v) echo $v, ' '; echo "\n";
foreach (new LimitIterator(new ArrayIterator($a), 1, 1) as $k => $v) echo "$k=$v "; echo "\n";
foreach (new CallbackFilterIterator(new ArrayIterator($a), fn($v, $k) => $k !== 'y') as $k => $v) echo "$k=$v "; echo "\n";
foreach (new IteratorIterator(new ArrayIterator($a)) as $k => $v) echo "$k=$v "; echo "\n";
foreach (new AppendIterator() as $v) echo "never"; $ap = new AppendIterator; $ap->append(new ArrayIterator([1])); $ap->append(new ArrayIterator([2])); foreach ($ap as $k => $v) echo "$k=$v "; echo "\n";
$ci = new CachingIterator(new ArrayIterator($a)); foreach ($ci as $k => $v) { echo "$k=$v", $ci->hasNext() ? ',' : '.'; } echo "\n";
foreach (new NoRewindIterator(new ArrayIterator([1, 2])) as $v) echo $v; echo "\n";
$inf = new InfiniteIterator(new ArrayIterator([1, 2])); $n = 0; foreach ($inf as $v) { echo $v; if (++$n >= 5) break; } echo "\n";
foreach (new RecursiveIteratorIterator(new RecursiveArrayIterator([1, [2, [3]], 4])) as $k => $v) echo "$k=$v "; echo "\n";
class Deep extends RecursiveIteratorIterator { function beginChildren(): void { echo "<"; } function endChildren(): void { echo ">"; } }
foreach (new Deep(new RecursiveArrayIterator([1, [2, [3]], 4])) as $v) echo $v; echo "\n";
$gen = (function () { yield 'a' => 1; yield 'b' => 2; })(); foreach ($gen as $k => $v) echo "$k=$v "; echo "\n";
$it = new ArrayIterator([1, 2, 3]); foreach ($it as $v) { if ($v == 2) break; } var_dump($it->current(), $it->key());
foreach ($it as $v) echo $v; echo "\n";
$e = new ArrayIterator([]); foreach ($e as $v) echo "never"; echo "empty ok\n";
try { foreach (new ArrayIterator([1]) as &$r) {} } catch (Error $ex) { echo get_class($ex), ": ", $ex->getMessage(), "\n"; }
$bi = new ArrayIterator([1, 2, 3]); foreach ($bi as &$r) { $r *= 2; } unset($r); var_dump($bi->getArrayCopy());
$bo = new ArrayObject(['k' => 1]); foreach ($bo as $k => &$r) { $r = "$k!"; } unset($r); var_dump($bo->getArrayCopy());
try { foreach ((function () { yield 1; })() as &$r) {} } catch (Exception $ex) { echo get_class($ex), ": ", $ex->getMessage(), "\n"; }
class U implements Iterator { function rewind(): void {} function valid(): bool { return false; } function current(): mixed { return 1; } function key(): mixed { return 1; } function next(): void {} }
try { foreach (new U as &$r) {} } catch (Error $ex) { echo get_class($ex), ": ", $ex->getMessage(), "\n"; }
$ao = new ArrayObject([1]); $it = @new ArrayIterator($ao); $it[] = 2; var_dump(count($ao)); $ao2 = @new ArrayObject($ao); $ao2["k"] = 3; var_dump(count($ao), $ao["k"]); $ai = $ao->getIterator(); $ai["z"] = 9; var_dump($ao["z"]); $ao["w"] = 1; var_dump(count($ai));
foreach ($ao as $k => $v) { echo "$k=$v "; } echo "\n"; var_dump(iterator_to_array($ao));
class MyIt extends ArrayIterator {} $ao->setIteratorClass('MyIt'); $mi = $ao->getIterator(); var_dump(get_class($mi), count($mi)); $mi['q'] = 5; var_dump($ao['q']);
$c = clone $ai; $c['cc'] = 1; var_dump(isset($ao['cc']));
var_dump($ai == $ao, $ai->getArrayCopy() === $ao->getArrayCopy());
